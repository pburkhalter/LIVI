//! A session that takes part in calls (CpConfig::calls, LIVI_CP_CALLS=1 in the helper; crate::calls). The sessions in
//! tests/bringup.rs run without.

use tokio::sync::mpsc;

use iap2_csm::CsmMessage;
use iap2_csm::messages::authentication::*;
use iap2_csm::messages::car_play::*;
use iap2_csm::messages::communications::*;
use iap2_csm::messages::identification::*;
use livi_runtime::bringup::{BringupEvent, CpConfig, run_accessory};
use livi_runtime::framing::frame_msg_id;
use livi_runtime::ident::{Identity, Transport};
use livi_runtime::vehicle::{Fuels, Plugs, Vehicle};
use livi_runtime::{AsyncAuth, ChannelError, ControlChannel};

struct PairChannel {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

fn pair() -> (PairChannel, PairChannel) {
    let (a_tx, a_rx) = mpsc::unbounded_channel();
    let (b_tx, b_rx) = mpsc::unbounded_channel();
    (
        PairChannel { tx: a_tx, rx: b_rx },
        PairChannel { tx: b_tx, rx: a_rx },
    )
}

impl ControlChannel for PairChannel {
    async fn send(&mut self, frame: Vec<u8>) -> Result<(), ChannelError> {
        self.tx.send(frame).map_err(|_| ChannelError::Closed)
    }
    async fn recv(&mut self) -> Option<Vec<u8>> {
        self.rx.recv().await
    }
}

impl PairChannel {
    async fn expect(&mut self, msg_id: u16) -> Vec<u8> {
        let frame = self.recv().await.expect("phone side channel closed");
        assert_eq!(
            frame_msg_id(&frame),
            Some(msg_id),
            "unexpected accessory message"
        );
        frame
    }
}

struct MockAuth;

impl AsyncAuth for MockAuth {
    async fn read_certificate(&mut self) -> Result<Vec<u8>, String> {
        Ok(vec![1])
    }
    async fn sign(&mut self, challenge: Vec<u8>) -> Result<Vec<u8>, String> {
        Ok(challenge)
    }
    async fn protocol_major(&mut self) -> Result<u8, String> {
        Ok(3)
    }
}

fn identity() -> Identity {
    Identity {
        name: "LIVI".into(),
        ssid: "LIVI".into(),
        bt_mac: [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33],
        fuels: Fuels::parse("Gasoline"),
        plugs: Plugs::default(),
        theme: None,
    }
}

fn cp_config() -> CpConfig {
    CpConfig {
        device_id: None,
        ap_mac: None,
        ap_on_air: None,
        wifi_iface: "nonexistent0".into(),
        ssid: "LIVI".into(),
        passphrase: "12345678".into(),
        channel: livi_wifi::Channel::of_number(36),
        airplay_port: 7000,
        source_version: "950.7.1".into(),
        public_key: String::new(),
        transport: Transport::Wireless,
        av_iface: None,
        av_iface_late: None,
        available_current_ma: 500,
        on_cable: None,
        start_again: None,
        theme: None,
        calls: true,
    }
}

fn names(list: &[u8]) -> Vec<u16> {
    list.chunks(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect()
}

/// Identification as the phone answers it (`refuse` first: rejects the lists of messages once), authentication,
/// subscriptions; returns the identifications the accessory sent
async fn bring_up(
    phone: &mut PairChannel,
    rx: &mut mpsc::Receiver<BringupEvent>,
    refuse: bool,
) -> Vec<IdentificationInformation> {
    let mut sent = Vec::new();
    phone.send(StartIdentification {}.encode()).await.unwrap();
    sent.push(IdentificationInformation::decode(&phone.expect(0x1D01).await).unwrap());
    if refuse {
        let mut rejected =
            IdentificationRejected::decode(&[0x40, 0x40, 0x00, 0x06, 0x1D, 0x03]).unwrap();
        rejected.messages_sent_by_accessory = true;
        phone.send(rejected.encode()).await.unwrap();
        sent.push(IdentificationInformation::decode(&phone.expect(0x1D01).await).unwrap());
    }
    phone
        .send(IdentificationAccepted {}.encode())
        .await
        .unwrap();
    assert_eq!(rx.recv().await, Some(BringupEvent::Identified));
    if refuse {
        assert_eq!(rx.recv().await, Some(BringupEvent::CallsRefused));
    }
    phone
        .send(RequestAuthenticationCertificate {}.encode())
        .await
        .unwrap();
    phone.expect(0xAA01).await;
    phone
        .send(RequestAuthenticationChallengeResponse { challenge: vec![1] }.encode())
        .await
        .unwrap();
    phone.expect(0xAA03).await;
    phone
        .send(AuthenticationSucceeded {}.encode())
        .await
        .unwrap();
    assert_eq!(rx.recv().await, Some(BringupEvent::Authenticated));
    sent
}

#[tokio::test]
async fn the_phone_hears_of_call_control_and_is_asked_for_its_lists() {
    let (accessory, mut phone) = pair();
    let (tx, mut rx) = mpsc::channel(32);
    let vehicle = Vehicle::default();
    tokio::spawn(run_accessory(
        accessory,
        MockAuth,
        identity(),
        cp_config(),
        tx,
        vehicle.feed(),
    ));
    let ident = bring_up(&mut phone, &mut rx, false).await;
    let sent = names(&ident[0].messages_sent_by_accessory);
    let received = names(&ident[0].messages_received_from_accessory);
    for m in [
        0x415A, 0x415B, 0x415C, 0x415D, 0x415F, 0x4160, 0x4170, 0x4154,
    ] {
        assert!(sent.contains(&m), "{m:04X} not named as sent");
    }
    for m in [0x4171, 0x4155] {
        assert!(received.contains(&m), "{m:04X} not named as received");
    }
    for expected in [0x5000, 0x5200, 0xAE00, 0x4157, 0x4154, 0x4170] {
        phone.expect(expected).await;
    }
    assert_eq!(rx.recv().await, Some(BringupEvent::Subscribed));
}

#[tokio::test]
async fn refused_lists_of_messages_leave_the_calls_out_and_carplay_goes_on() {
    let (accessory, mut phone) = pair();
    let (tx, mut rx) = mpsc::channel(32);
    let vehicle = Vehicle::default();
    tokio::spawn(run_accessory(
        accessory,
        MockAuth,
        identity(),
        cp_config(),
        tx,
        vehicle.feed(),
    ));
    let ident = bring_up(&mut phone, &mut rx, true).await;
    let again = names(&ident[1].messages_sent_by_accessory);
    assert!(!again.contains(&0x415A) && !again.contains(&0x4170));
    assert!(again.contains(&0x4154), "the call state stays");
    assert!(!names(&ident[1].messages_received_from_accessory).contains(&0x4171));
    // the subscriptions of a session without calls: no request for the lists
    for expected in [0x5000, 0x5200, 0xAE00, 0x4157, 0x4154] {
        phone.expect(expected).await;
    }
    assert_eq!(rx.recv().await, Some(BringupEvent::Subscribed));
    // and no orders: the phone was never told of them
    vehicle.push_call("- dial +41 79 000 00 01").unwrap();
    vehicle.push_call("- end").unwrap();
    let nothing =
        tokio::time::timeout(std::time::Duration::from_millis(300), phone.rx.recv()).await;
    assert!(
        nothing.is_err(),
        "sent after the refusal: {:?}",
        nothing.ok().flatten().as_deref().and_then(frame_msg_id)
    );
}

#[tokio::test]
async fn orders_reach_the_phone_they_are_meant_for() {
    let (accessory, mut phone) = pair();
    let (tx, mut rx) = mpsc::channel(32);
    let vehicle = Vehicle::default();
    tokio::spawn(run_accessory(
        accessory,
        MockAuth,
        identity(),
        cp_config(),
        tx,
        vehicle.feed(),
    ));
    bring_up(&mut phone, &mut rx, false).await;
    for expected in [0x5000, 0x5200, 0xAE00, 0x4157, 0x4154, 0x4170] {
        phone.expect(expected).await;
    }
    // the session listens for orders from here on; one before has nobody to go to
    assert_eq!(rx.recv().await, Some(BringupEvent::Subscribed));

    vehicle.push_call("- dial +41 79 000 00 01").unwrap();
    let dial = InitiateCall::decode(&phone.expect(0x415A).await).unwrap();
    assert_eq!(dial.destination_id.as_deref(), Some("+41790000001"));
    assert_eq!((dial.call_type, dial.service), (0, Some(1)));

    let names = DeviceTransportIdentifierNotification {
        bluetooth_transport_id: "AA:BB:CC:DD:EE:FF".into(),
        usb_transport_id: String::new(),
    };
    phone.send(names.encode()).await.unwrap();
    while !matches!(
        rx.recv().await,
        Some(BringupEvent::Incoming { msg_id: 0x4E0E, .. })
    ) {}

    vehicle.push_call("11:22:33:44:55:66 mute on").unwrap();
    vehicle.push_call("aa:bb:cc:dd:ee:ff end").unwrap();
    let end = EndCall::decode(&phone.expect(0x415C).await).unwrap();
    assert_eq!(
        (end.action, end.call_uuid),
        (0, None),
        "the other phone's mute went nowhere"
    );
    vehicle.push_call("- accept").unwrap();
    vehicle.push_call("- mute on").unwrap();
    phone.expect(0x415B).await;
    assert!(
        MuteStatusUpdate::decode(&phone.expect(0x4160).await)
            .unwrap()
            .mute_status
    );
    assert!(vehicle.push_call("- dial nobody").is_err());
}
