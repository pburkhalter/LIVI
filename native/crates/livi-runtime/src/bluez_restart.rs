//! A bluetoothd that starts anew (after a crash, or by hand: `systemctl restart bluetooth`) knows nothing of what the
//! helper registered with the one before: the profiles (iAP2, CarPlay, hands-free, Android Auto), the pairing agent, the
//! BLE advertisement, the media player, and the settings of the controllers. A phone would connect and find nothing.
//! The helper follows org.bluez's owner (the bus's NameOwnerChanged) and registers all of it with every new bluetoothd,
//! in the same process: its objects stay on the bus, only BlueZ is asked again.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use zbus::message::Type;
use zbus::{Connection, MatchRule, MessageStream};

use crate::bt::{self, Held, Setup};
use crate::shared_sock::Track;

const BLUEZ: &str = "org.bluez";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// What the helper registered with bluetoothd, in the order it did
#[derive(Clone)]
pub enum Part {
    Agent,
    CarPlay,
    Hfp,
    AndroidAuto,
    Advertisement,
    /// with what plays now, which a new bluetoothd learns at once
    Player {
        status: Arc<Mutex<String>>,
        track: Arc<Mutex<Track>>,
    },
}

impl Part {
    fn name(&self) -> &'static str {
        match self {
            Part::Agent => "agent",
            Part::CarPlay => "CarPlay",
            Part::Hfp => "hands-free",
            Part::AndroidAuto => "Android Auto",
            Part::Advertisement => "advertisement",
            Part::Player { .. } => "player",
        }
    }

    /// Registered before the controller is set up, as at the start
    fn before_settings(&self) -> bool {
        matches!(self, Part::Agent | Part::CarPlay)
    }
}

/// The interfaces the controller must show before the parts can be registered on it
fn needs(parts: &[Part]) -> Vec<&'static str> {
    let mut needs = vec![bt::ADAPTER1];
    if parts.iter().any(|p| matches!(p, Part::Player { .. })) {
        needs.push(bt::MEDIA1);
    }
    if parts.iter().any(|p| matches!(p, Part::Advertisement)) {
        needs.push(bt::ADVERTISING1);
    }
    needs
}

/// Whether a new owner of org.bluez is a bluetoothd the helper has not registered with
fn registers_anew(registered_with: Option<&str>, new_owner: &str) -> bool {
    !new_owner.is_empty() && registered_with != Some(new_owner)
}

/// The owner of org.bluez now, a unique name
async fn owner(conn: &Connection) -> Option<String> {
    let reply = conn
        .call_method(Some(DBUS), DBUS_PATH, Some(DBUS), "GetNameOwner", &(BLUEZ,))
        .await
        .ok()?;
    reply.body().deserialize::<String>().ok()
}

/// org.bluez's owner, followed from before the helper's first registration
pub struct Restarts {
    owners: MessageStream,
    before: Option<String>,
}

impl Restarts {
    pub(crate) async fn follow(conn: &Connection) -> zbus::Result<Self> {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(DBUS)?
            .interface(DBUS)?
            .member("NameOwnerChanged")?
            .add_arg(BLUEZ)?
            .build();
        let owners = MessageStream::for_match_rule(rule, conn, None).await?;
        // bluetoothd started by the bus if it does not run yet, as the first registration would: its name has an owner
        // before anything is registered. When it cannot be started, the registrations fail and say why
        let _ = conn
            .call_method(
                Some(DBUS),
                DBUS_PATH,
                Some(DBUS),
                "StartServiceByName",
                &(BLUEZ, 0u32),
            )
            .await;
        let before = owner(conn).await;
        Ok(Self { owners, before })
    }
}

struct Inner {
    conn: Connection,
    setup: Setup,
    held: Held,
    parts: Mutex<Vec<Part>>,
    restarts: Mutex<Option<Restarts>>,
}

/// Everything the helper registered with bluetoothd and the setup of its controller, for the next bluetoothd
#[derive(Clone)]
pub struct Registry(Arc<Inner>);

impl Registry {
    pub(crate) fn new(
        conn: Connection,
        setup: Setup,
        held: Held,
        parts: Vec<Part>,
        restarts: Restarts,
    ) -> Self {
        Self(Arc::new(Inner {
            conn,
            setup,
            held,
            parts: Mutex::new(parts),
            restarts: Mutex::new(Some(restarts)),
        }))
    }

    pub fn conn(&self) -> &Connection {
        &self.0.conn
    }

    pub fn adapter(&self) -> &str {
        &self.0.setup.adapter
    }

    pub(crate) fn add(&self, part: Part) {
        self.0.parts.lock().unwrap().push(part);
    }

    fn parts(&self) -> Vec<Part> {
        self.0.parts.lock().unwrap().clone()
    }

    /// Follows org.bluez's owner as long as the system bus is there, and registers everything anew with each bluetoothd
    /// that comes. Spawned once all `bt::start_*` are done; what comes meanwhile waits in the stream.
    pub async fn follow(self) {
        let Some(Restarts { mut owners, before }) = self.0.restarts.lock().unwrap().take() else {
            return;
        };
        let mut current = owner(&self.0.conn).await;
        if let Some(now) = current.clone().filter(|now| before.as_deref() != Some(now)) {
            // another bluetoothd than the one at the first registration: some of it went to the one before
            self.register_again(&now).await;
        }
        while let Some(msg) = owners.next().await {
            let change = msg.and_then(|m| m.body().deserialize::<(String, String, String)>());
            let (_, old, new) = match change {
                Ok(change) => change,
                Err(e) => {
                    eprintln!("[bt] a change of BlueZ's owner could not be read: {e}");
                    continue;
                }
            };
            if new.is_empty() {
                if current.as_deref() == Some(old.as_str()) {
                    println!("[bt] BlueZ is gone, the helper registers with the next one");
                    current = None;
                }
            } else if registers_anew(current.as_deref(), &new) {
                self.register_again(&new).await;
                current = Some(new);
            }
        }
        eprintln!("[bt] the system bus closed: a new bluetoothd would not get LIVI's profiles");
    }

    /// The parts registered again, in the order of the start, with the controller's settings between them
    async fn register_again(&self, owner: &str) {
        println!("[bt] BlueZ started again: the helper registers anew");
        let (conn, setup) = (&self.0.conn, &self.0.setup);
        let parts = self.parts();
        let path = format!("/org/bluez/{}", setup.adapter);
        // as long as it takes: after the chip's own restart its controller may come much later than bluetoothd
        if let Err(e) = bt::adapter_ready(conn, &path, &needs(&parts), None).await {
            eprintln!(
                "[bt] BlueZ {owner} shows no {}: nothing registered anew ({e})",
                setup.adapter
            );
            return;
        }
        let mut done = Vec::new();
        let (first, rest): (Vec<_>, Vec<_>) = parts.iter().partition(|p| p.before_settings());
        for part in first {
            self.again(part, &mut done).await;
        }
        match bt::configure_again(conn, setup, &self.0.held).await {
            Ok(()) => done.push("settings"),
            Err(e) => eprintln!("[bt] the settings of {} not made anew: {e}", setup.adapter),
        }
        for part in rest {
            self.again(part, &mut done).await;
        }
        println!(
            "[bt] registered anew with BlueZ {owner}: {}",
            done.join(", ")
        );
    }

    async fn again(&self, part: &Part, done: &mut Vec<&'static str>) {
        match bt::register_part(&self.0.conn, &self.0.setup.adapter, part).await {
            Ok(()) => done.push(part.name()),
            // as at the start: another program (the audio daemon, oFono) was quicker with the new bluetoothd
            Err(e) if matches!(part, Part::Hfp) => {
                println!("[hfp] HF profile held by another program now ({e})")
            }
            Err(e) => eprintln!("[bt] {} not registered anew: {e}", part.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use tokio::sync::Notify;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue};

    use super::*;
    use crate::bt::{Pairing, Wireless};

    #[test]
    fn only_a_bluetoothd_the_helper_did_not_register_with_gets_it_all_again() {
        // a crash: the old owner goes, then a new one comes
        assert!(!registers_anew(Some(":1.12"), ""));
        assert!(registers_anew(None, ":1.60"));
        // handed over directly
        assert!(registers_anew(Some(":1.12"), ":1.60"));
        // the one the helper registered with, seen late (it was started for the helper)
        assert!(!registers_anew(Some(":1.12"), ":1.12"));
    }

    #[test]
    fn a_player_and_an_advertisement_wait_for_their_interfaces() {
        assert_eq!(needs(&[Part::Agent, Part::CarPlay]), [bt::ADAPTER1]);
        let player = Part::Player {
            status: Default::default(),
            track: Default::default(),
        };
        assert_eq!(
            needs(&[Part::Hfp, player, Part::Advertisement]),
            [bt::ADAPTER1, bt::MEDIA1, bt::ADVERTISING1]
        );
    }

    /// What the stand-in for bluetoothd was asked
    #[derive(Clone, Default)]
    struct Asked {
        calls: Arc<Mutex<Vec<String>>>,
        came: Arc<Notify>,
        owner: Arc<Mutex<Option<String>>>,
    }

    impl Asked {
        fn note(&self, call: String) {
            self.calls.lock().unwrap().push(call);
            self.came.notify_waiters();
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.calls.lock().unwrap())
        }

        /// Waits until all of `calls` were asked since the last take, at most 5 s
        async fn until(&self, calls: &[String]) -> Vec<String> {
            let all = async {
                loop {
                    let came = self.came.notified();
                    tokio::pin!(came);
                    came.as_mut().enable();
                    let now = self.calls.lock().unwrap().clone();
                    if calls.iter().all(|c| now.contains(c)) {
                        return now;
                    }
                    came.await;
                }
            };
            tokio::time::timeout(Duration::from_secs(5), all)
                .await
                .unwrap_or_else(|_| panic!("not asked {calls:?}: {:?}", self.calls.lock().unwrap()))
        }
    }

    struct Bus(Asked);

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl Bus {
        fn get_name_owner(&self, name: String) -> zbus::fdo::Result<String> {
            assert_eq!(name, BLUEZ);
            self.0
                .owner
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| zbus::fdo::Error::NameHasNoOwner(name))
        }
        fn start_service_by_name(&self, _name: String, _flags: u32) -> u32 {
            2
        }
    }

    struct Manager(Asked);

    #[zbus::interface(name = "org.bluez.ProfileManager1")]
    impl Manager {
        fn register_profile(
            &self,
            _path: OwnedObjectPath,
            uuid: String,
            _options: HashMap<String, OwnedValue>,
        ) {
            self.0.note(format!("profile {uuid}"));
        }
    }

    struct Agents(Asked);

    #[zbus::interface(name = "org.bluez.AgentManager1")]
    impl Agents {
        fn register_agent(&self, _path: OwnedObjectPath, _capability: String) {
            self.0.note("agent".into());
        }
        fn request_default_agent(&self, _path: OwnedObjectPath) {
            self.0.note("default agent".into());
        }
    }

    struct Adapter(Asked);

    #[zbus::interface(name = "org.bluez.Adapter1")]
    impl Adapter {
        #[zbus(property)]
        fn address(&self) -> String {
            "00:11:22:33:44:55".into()
        }
        #[zbus(property)]
        fn alias(&self) -> String {
            "bluez".into()
        }
        #[zbus(property)]
        fn set_alias(&mut self, value: String) {
            self.0.note(format!("Alias={value}"));
        }
        #[zbus(property)]
        fn powered(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn set_powered(&mut self, value: bool) {
            self.0.note(format!("Powered={value}"));
        }
        #[zbus(property)]
        fn discoverable(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn set_discoverable(&mut self, value: bool) {
            self.0.note(format!("Discoverable={value}"));
        }
        #[zbus(property)]
        fn discoverable_timeout(&self) -> u32 {
            180
        }
        #[zbus(property)]
        fn set_discoverable_timeout(&mut self, value: u32) {
            self.0.note(format!("DiscoverableTimeout={value}"));
        }
        #[zbus(property)]
        fn pairable(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn set_pairable(&mut self, value: bool) {
            self.0.note(format!("Pairable={value}"));
        }
        #[zbus(property)]
        fn pairable_timeout(&self) -> u32 {
            0
        }
        #[zbus(property)]
        fn set_pairable_timeout(&mut self, value: u32) {
            self.0.note(format!("PairableTimeout={value}"));
        }
    }

    struct Media(Asked);

    #[zbus::interface(name = "org.bluez.Media1")]
    impl Media {
        fn register_player(&self, _path: OwnedObjectPath, props: HashMap<String, OwnedValue>) {
            let status = props
                .get("PlaybackStatus")
                .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
                .unwrap_or_default();
            self.0.note(format!("player {status}"));
        }
    }

    struct Advertising(Asked);

    #[zbus::interface(name = "org.bluez.LEAdvertisingManager1")]
    impl Advertising {
        fn register_advertisement(
            &self,
            _path: OwnedObjectPath,
            _options: HashMap<String, OwnedValue>,
        ) {
            self.0.note("advertisement".into());
        }
    }

    const HCI0: &str = "/org/bluez/hci0";

    async fn controller_comes(bluez: &Connection, asked: &Asked) {
        let server = bluez.object_server();
        server.at(HCI0, Adapter(asked.clone())).await.unwrap();
        server.at(HCI0, Media(asked.clone())).await.unwrap();
        server.at(HCI0, Advertising(asked.clone())).await.unwrap();
    }

    async fn controller_goes(bluez: &Connection) {
        let server = bluez.object_server();
        server.remove::<Adapter, _>(HCI0).await.unwrap();
        server.remove::<Media, _>(HCI0).await.unwrap();
        server.remove::<Advertising, _>(HCI0).await.unwrap();
    }

    /// As the bus daemon sends it: its own name as the sender
    async fn owner_changed(bluez: &Connection, old: &str, new: &str) {
        let msg = zbus::Message::signal(DBUS_PATH, DBUS, "NameOwnerChanged")
            .unwrap()
            .sender(DBUS)
            .unwrap()
            .build(&(BLUEZ, old, new))
            .unwrap();
        bluez.send(&msg).await.unwrap();
    }

    /// The helper against a stand-in for bluetoothd over a socket pair: what it registers at the start, nothing more for
    /// the bluetoothd it has, everything again for a new one, in-process
    #[tokio::test]
    async fn a_new_bluetoothd_gets_everything_again() {
        let asked = Asked::default();
        *asked.owner.lock().unwrap() = Some(":1.5".into());
        let (ours, theirs) = tokio::net::UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let bluez = zbus::connection::Builder::unix_stream(theirs)
            .server(guid)
            .unwrap()
            .p2p()
            .serve_at("/", zbus::fdo::ObjectManager)
            .unwrap()
            .serve_at(DBUS_PATH, Bus(asked.clone()))
            .unwrap()
            .serve_at("/org/bluez", Manager(asked.clone()))
            .unwrap()
            .serve_at("/org/bluez", Agents(asked.clone()))
            .unwrap()
            .build();
        let helper = zbus::connection::Builder::unix_stream(ours).p2p().build();
        let (bluez, helper) = tokio::try_join!(bluez, helper).unwrap();
        controller_comes(&bluez, &asked).await;

        let setup = Setup {
            adapter: "hci0".into(),
            alias: "LIVI".into(),
            wireless: Wireless {
                carplay: true,
                android_auto: true,
            },
            pairing: Pairing::Livi,
            pairing_seconds: 0,
        };
        let (_conn, _incoming, _held, registry) = bt::start_on(helper, &setup).await.unwrap();
        bt::start_hfp(&registry, crate::hfp::Hfp::default())
            .await
            .unwrap();
        let _aa = bt::start_aa(&registry).await.unwrap();
        bt::start_ble_ad(&registry, "LIVI").await.unwrap();
        let player = bt::start_media_player(&registry, Default::default())
            .await
            .unwrap();
        player.set_status("Paused").await;
        let profile = |uuid: &str| format!("profile {uuid}");
        let mut everything = [
            "agent",
            "default agent",
            "advertisement",
            "Alias=LIVI",
            "DiscoverableTimeout=0",
            "Powered=true",
            "Discoverable=true",
            "PairableTimeout=0",
            "Pairable=true",
        ]
        .map(String::from)
        .to_vec();
        for uuid in [
            bt::IAP_SERVER_UUID,
            bt::IAP_CLIENT_UUID,
            bt::CARPLAY_SERVICE_UUID,
            bt::HFP_HF_UUID,
            bt::AA_UUID,
        ] {
            everything.push(profile(uuid));
        }
        let at_start = [everything.clone(), vec!["player Playing".to_string()]].concat();
        asked.until(&at_start).await;
        asked.take();
        tokio::spawn(registry.follow());

        // the bluetoothd the helper registered with, seen late (the bus started it for the helper): nothing again
        owner_changed(&bluez, "", ":1.5").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(asked.take(), Vec::<String>::new());

        // a crash: the old one goes with its controller, a new one comes, its controller a moment later
        controller_goes(&bluez).await;
        *asked.owner.lock().unwrap() = None;
        owner_changed(&bluez, ":1.5", "").await;
        *asked.owner.lock().unwrap() = Some(":1.9".into());
        owner_changed(&bluez, "", ":1.9").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        controller_comes(&bluez, &asked).await;
        // with what plays now, not what played at the start
        let again = [everything, vec!["player Paused".to_string()]].concat();
        let again = asked.until(&again).await;
        // in the order of the start: agent and CarPlay before the settings, the rest after them
        let at = |call: &str| again.iter().position(|c| c == call).unwrap();
        assert!(at("agent") < at("Alias=LIVI"));
        assert!(at(&profile(bt::IAP_SERVER_UUID)) < at("Alias=LIVI"));
        assert!(at("Pairable=true") < at(&profile(bt::HFP_HF_UUID)));
        assert!(at("Pairable=true") < at("player Paused"));
    }
}
