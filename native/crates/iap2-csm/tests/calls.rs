//! Call control and the call lists against the bytes published for them: the StartListUpdates request a car
//! answered (ProManiFest2122/mhi2-k3346-carplay-vc, tests/k3346_tel_test.c, "verified_start", from xPaiiN's capture),
//! the InitiateCall bytes of the same tests, and the layouts of luka-dev/mib2q-carplay-rgi
//! (docs/cluster-and-rgi/IAP2_COMMUNICATIONS.md). The lists below are made up; only their layout is the phone's.

use iap2_csm::messages::communications::*;
use iap2_csm::{CsmMessage, put_param};

fn frame(msg_id: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0x40, 0x40];
    f.extend_from_slice(&(payload.len() as u16 + 6).to_be_bytes());
    f.extend_from_slice(&msg_id.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

fn all_recent_fields() -> RecentsListProperties {
    RecentsListProperties {
        index: true,
        remote_id: true,
        display_name: true,
        label: true,
        address_book_id: true,
        service: true,
        kind: true,
        unix_timestamp: true,
        duration: true,
        occurrences: true,
    }
}

fn all_favorite_fields() -> FavoritesListProperties {
    FavoritesListProperties {
        index: true,
        remote_id: true,
        display_name: true,
        label: true,
        address_book_id: true,
        service: true,
    }
}

#[test]
fn start_list_updates_is_the_request_a_car_got_an_answer_to() {
    let verified: &[u8] = &[
        0x00, 0x2c, 0x00, 0x01, //
        0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0x02, 0x00, 0x04, 0x00,
        0x03, 0x00, 0x04, 0x00, 0x04, 0x00, 0x04, 0x00, 0x05, 0x00, 0x04, 0x00, 0x06, 0x00, 0x04,
        0x00, 0x07, 0x00, 0x04, 0x00, 0x08, 0x00, 0x04, 0x00, 0x09, //
        0x00, 0x06, 0x00, 0x03, 0x00, 0x19, //
        0x00, 0x05, 0x00, 0x04, 0x01, //
        0x00, 0x1c, 0x00, 0x06, //
        0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0x02, 0x00, 0x04, 0x00,
        0x03, 0x00, 0x04, 0x00, 0x04, 0x00, 0x04, 0x00, 0x05, //
        0x00, 0x06, 0x00, 0x08, 0x00, 0x00,
    ];
    assert_eq!(verified.len(), 89);
    let m = StartListUpdates {
        recents_available: false,
        recents_properties: Some(all_recent_fields()),
        recents_count: false,
        recents_max: Some(25),
        recents_combine: Some(true),
        favorites_available: false,
        favorites_properties: Some(all_favorite_fields()),
        favorites_count: false,
        favorites_max: Some(0),
    };
    assert_eq!(m.encode(), frame(0x4170, verified));
    assert_eq!(StartListUpdates::decode(&m.encode()).unwrap(), m);
}

#[test]
fn initiate_call_carries_type_number_and_service() {
    let m = InitiateCall {
        call_type: 0,
        destination_id: Some("+00123".into()),
        service: Some(1),
        address_book_id: None,
    };
    let payload = [
        &[0x00, 0x05, 0x00, 0x00, 0x00][..],
        &[
            0x00, 0x0b, 0x00, 0x01, b'+', b'0', b'0', b'1', b'2', b'3', 0x00,
        ],
        &[0x00, 0x05, 0x00, 0x02, 0x01],
    ]
    .concat();
    assert_eq!(m.encode(), frame(0x415A, &payload));
    assert_eq!(InitiateCall::decode(&m.encode()).unwrap(), m);
}

#[test]
fn redial_needs_no_number() {
    let m = InitiateCall {
        call_type: 2,
        destination_id: None,
        service: None,
        address_book_id: None,
    };
    assert_eq!(m.encode(), frame(0x415A, &[0x00, 0x05, 0x00, 0x00, 0x02]));
}

#[test]
fn accept_end_hold_and_mute_put_their_value_first() {
    let accept = AcceptCall {
        action: 0,
        call_uuid: None,
    };
    assert_eq!(
        accept.encode(),
        frame(0x415B, &[0x00, 0x05, 0x00, 0x00, 0x00])
    );
    let end_all = EndCall {
        action: 1,
        call_uuid: None,
    };
    assert_eq!(
        end_all.encode(),
        frame(0x415C, &[0x00, 0x05, 0x00, 0x00, 0x01])
    );
    let hold = HoldStatusUpdate {
        hold_status: true,
        call_uuid: None,
    };
    assert_eq!(
        hold.encode(),
        frame(0x415F, &[0x00, 0x05, 0x00, 0x00, 0x01])
    );
    let mute = MuteStatusUpdate { mute_status: false };
    assert_eq!(
        mute.encode(),
        frame(0x4160, &[0x00, 0x05, 0x00, 0x00, 0x00])
    );
    assert_eq!(SwapCalls {}.encode(), frame(0x415D, &[]));
}

#[test]
fn a_call_uuid_follows_the_action() {
    let m = EndCall {
        action: 0,
        call_uuid: Some("ab".into()),
    };
    let payload = [
        &[0x00, 0x05, 0x00, 0x00, 0x00][..],
        &[0x00, 0x07, 0x00, 0x01, b'a', b'b', 0x00],
    ]
    .concat();
    assert_eq!(m.encode(), frame(0x415C, &payload));
    assert_eq!(EndCall::decode(&m.encode()).unwrap(), m);
}

fn text(id: u16, s: &str) -> Vec<u8> {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    let mut out = Vec::new();
    put_param(&mut out, id, &b);
    out
}

fn param(id: u16, b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_param(&mut out, id, b);
    out
}

fn recent(index: u16, number: &str, name: &str, kind: u8, when: u64, count: u8) -> Vec<u8> {
    let fields = [
        param(0, &index.to_be_bytes()),
        text(1, number),
        text(2, name),
        text(3, "mobile"),
        text(4, "00000000-0000-0000-0000-000000000000"),
        param(5, &[1]),
        param(6, &[kind]),
        param(7, &when.to_be_bytes()),
        param(9, &[count]),
    ]
    .concat();
    param(1, &fields)
}

#[test]
fn a_list_comes_in_parts_head_first() {
    let head = [param(0, &[1]), param(2, &2u16.to_be_bytes())].concat();
    let m = ListUpdate::decode(&frame(0x4171, &head)).unwrap();
    assert_eq!(m.recents_available, Some(true));
    assert_eq!(m.recents_count, Some(2));
    assert!(m.recents.is_empty());
    assert_eq!(m.favorites_count, None);

    let entries = [
        recent(0, "+41790000001", "Test Eins", 2, 1_790_000_000, 2),
        recent(1, "+41790000002", "+41 79 000 00 02", 3, 1_789_990_000, 1),
    ]
    .concat();
    let m = ListUpdate::decode(&frame(0x4171, &entries)).unwrap();
    assert_eq!(m.recents.len(), 2);
    assert_eq!(m.recents_count, None);
    let first = &m.recents[0];
    assert_eq!(first.index, Some(0));
    assert_eq!(first.remote_id.as_deref(), Some("+41790000001"));
    assert_eq!(first.display_name.as_deref(), Some("Test Eins"));
    assert_eq!(first.label.as_deref(), Some("mobile"));
    assert_eq!(first.kind, Some(2));
    assert_eq!(first.unix_timestamp, Some(1_790_000_000));
    assert_eq!(first.occurrences, Some(2));
    assert_eq!(m.recents[1].kind, Some(3));
    assert_eq!(m.encode(), frame(0x4171, &entries));
}

#[test]
fn favourites_have_their_own_ids() {
    let fav = [
        &[0x00, 0x06, 0x00, 0x00, 0x00, 0x00][..],
        &text(1, "000-000-016"),
        &text(2, "Test Zwei"),
        &param(5, &[1]),
    ]
    .concat();
    let payload = [
        param(5, &[1]),
        param(6, &fav),
        param(7, &1u16.to_be_bytes()),
    ]
    .concat();
    let m = ListUpdate::decode(&frame(0x4171, &payload)).unwrap();
    assert!(m.recents.is_empty());
    assert_eq!(m.favorites_available, Some(true));
    assert_eq!(m.favorites_count, Some(1));
    assert_eq!(m.favorites[0].remote_id.as_deref(), Some("000-000-016"));
    assert_eq!(m.favorites[0].display_name.as_deref(), Some("Test Zwei"));
    assert_eq!(m.encode(), frame(0x4171, &payload));
}

#[test]
fn an_odd_entry_does_not_take_the_others_with_it() {
    let broken = param(1, &[0x00, 0x05, 0x00, 0x00, 0x07]);
    let payload = [broken, recent(1, "+41790000002", "Test", 1, 1, 1)].concat();
    let m = ListUpdate::decode(&frame(0x4171, &payload)).unwrap();
    assert_eq!(m.recents.len(), 1);
    assert_eq!(m.recents[0].index, Some(1));
}

#[test]
fn a_count_of_the_wrong_size_is_an_error() {
    let payload = param(2, &[3]);
    assert!(ListUpdate::decode(&frame(0x4171, &payload)).is_err());
}
