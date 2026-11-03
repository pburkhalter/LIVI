//! Calls through the CarPlay session (iAP2 Communications), for a UI of the car's own beside CarPlay: dialling,
//! answering, ending, holding, muting and swapping calls, and the phone's recent and favourite calls. Off unless
//! `LIVI_CP_CALLS=1`; then the identification names the messages, the session asks for the lists and takes orders from
//! the core (`call` on the helper's socket). The tones of the keypad go as CarPlay telephony keys, not from here.

use iap2_csm::CsmMessage;
use iap2_csm::messages::communications::*;
use serde_json::{Value, json};

pub const ENV: &str = "LIVI_CP_CALLS";

pub fn enabled() -> bool {
    std::env::var(ENV).is_ok_and(|v| v == "1")
}

/// Looking at the phone's call messages without taking part: every 0x41xx message in the log, its texts masked, and the
/// subscriptions to the call state and the communications ask for every field. Nothing more is sent or named in the
/// identification; independent of LIVI_CP_CALLS
pub const LOG_ENV: &str = "LIVI_IAP2_TRACE";

pub fn log_enabled() -> bool {
    std::env::var(LOG_ENV).is_ok_and(|v| v == "1")
}

/// A text as the log shows it: its length, and the last two digits when it has digits (enough to tell calls apart,
/// never a name or a whole number)
fn masked(text: &str) -> String {
    let digits: Vec<char> = text.chars().filter(char::is_ascii_digit).collect();
    match digits.len() {
        0 => format!("text<{}>", text.chars().count()),
        n => format!(
            "text<{}…{}>",
            text.chars().count(),
            digits[n.saturating_sub(2)..].iter().collect::<String>()
        ),
    }
}

/// A UTF-8 text ending in 0x00, with no other 0x00 (control characters too: masked rather than shown)
fn as_text(b: &[u8]) -> Option<&str> {
    let (last, body) = b.split_last()?;
    if *last != 0 || body.contains(&0) {
        return None;
    }
    std::str::from_utf8(body).ok()
}

/// Bytes that read as letters: a text without its 0x00, never shown as a number
fn looks_like_text(b: &[u8]) -> bool {
    b.iter().all(|c| c.is_ascii_graphic() || *c == b' ')
}

/// Groups within groups the log goes into; deeper ones only by their length
const DEPTH_MAX: usize = 3;

/// Parameters within a parameter, when its bytes are exactly that
fn as_group(b: &[u8]) -> Option<Vec<(u16, &[u8])>> {
    let params = iap2_csm::split_params(b);
    let used: usize = params.iter().map(|(_, p)| p.len() + 4).sum();
    (b.len() >= 4 && used == b.len() && params.iter().all(|(id, _)| *id < 64)).then_some(params)
}

fn value(b: &[u8], depth: usize) -> String {
    let int = |n: usize| b.iter().take(n).fold(0u64, |v, x| (v << 8) | u64::from(*x));
    match b.len() {
        0 => "flag".into(),
        1 => format!("u8 {}", b[0]),
        2 if as_text(b).is_none() && !looks_like_text(b) => format!("u16 {}", int(2)),
        _ => {
            if let Some(t) = as_text(b) {
                masked(t)
            } else if let Some(group) = as_group(b).filter(|_| depth < DEPTH_MAX) {
                let inner: Vec<String> = group
                    .iter()
                    .map(|(id, p)| format!("{id} {}", value(p, depth + 1)))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            } else if (b.len() == 4 || b.len() == 8) && !looks_like_text(b) {
                format!("u{} {}", b.len() * 8, int(b.len()))
            } else {
                format!("bytes<{}>", b.len())
            }
        }
    }
}

/// One line for the log of a message of call state, communications, call control or the call lists
/// (0x4154..=0x4172): its id and parameters, texts masked; None for other messages
pub fn describe(frame: &[u8]) -> Option<String> {
    let (msg_id, payload) = iap2_csm::frame_header(frame).ok()?;
    if !(0x4154..=0x4172).contains(&msg_id) {
        return None;
    }
    let params: Vec<String> = iap2_csm::split_params(payload)
        .iter()
        .map(|(id, p)| format!("{id} {}", value(p, 0)))
        .collect();
    Some(format!("0x{msg_id:04X}: {}", params.join(" | ")))
}

/// The subscriptions to the call state (0x4154) and the communications (0x4157) with every field the phone knows
/// (luka-dev, IAP2_COMMUNICATIONS.md: call state 0-4, 6-12; communications 0-17)
pub fn full_subscriptions() -> (Vec<u8>, Vec<u8>) {
    let start = |msg_id: u16, ids: &[u16]| {
        let mut params = Vec::new();
        for id in ids {
            iap2_csm::put_param(&mut params, *id, &[]);
        }
        let mut f = vec![0x40, 0x40];
        f.extend_from_slice(&(params.len() as u16 + 6).to_be_bytes());
        f.extend_from_slice(&msg_id.to_be_bytes());
        f.extend_from_slice(&params);
        f
    };
    let calls: Vec<u16> = (0..=12).filter(|id| *id != 5).collect();
    let comms: Vec<u16> = (0..=17).collect();
    (start(0x4154, &calls), start(0x4157, &comms))
}

/// What the accessory sends for it (call control 0x415A..0x4160, the lists 0x4170/0x4172) and receives (0x4171).
/// Call control needs StartCallStateUpdates and CallStateUpdate, which the identification names anyway.
pub const SENT: &[u16] = &[
    0x415A, 0x415B, 0x415C, 0x415D, 0x415F, 0x4160, 0x4170, 0x4172,
];
pub const RECEIVED: &[u16] = &[0x4171];

/// Longest number dialled; what the phone takes is longer, a typo is not
const NUMBER_MAX: usize = 32;
/// Entries kept of a list; under CarPlay the phone sends at most 12
const LIST_MAX: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    Dial(String),
    Redial,
    Voicemail,
    /// Accept the ringing call, holding an active one
    Accept,
    /// End the active call and accept the ringing one
    AcceptEnding,
    /// End a call: the one with this id, without one the frontmost (also while it rings or dials)
    End(Option<String>),
    EndAll,
    Hold(bool),
    Mute(bool),
    Swap,
}

fn on_off(word: Option<&str>) -> Result<bool, String> {
    match word {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ => Err("on or off".to_string()),
    }
}

/// The number as the phone gets it: digits, a leading +, * and #; spaces, dashes, dots and brackets go
pub fn dialable(text: &str) -> Option<String> {
    let number: String = text
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')' | '/'))
        .collect();
    let ok = !number.is_empty()
        && number.len() <= NUMBER_MAX
        && number
            .char_indices()
            .all(|(i, c)| c.is_ascii_digit() || c == '*' || c == '#' || (c == '+' && i == 0))
        && number.chars().any(|c| c.is_ascii_digit());
    ok.then_some(number)
}

impl Order {
    /// `dial <number>`, `redial`, `voicemail`, `accept`, `accept end`, `end [<call id>]`, `end all`,
    /// `hold on|off`, `mute on|off`, `swap`
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (verb, rest) = text.split_once(' ').unwrap_or((text, ""));
        let rest = rest.trim();
        let word = (!rest.is_empty()).then_some(rest);
        match (verb, word) {
            ("dial", Some(n)) => dialable(n)
                .map(Order::Dial)
                .ok_or_else(|| "not a number to dial".to_string()),
            ("redial", None) => Ok(Order::Redial),
            ("voicemail", None) => Ok(Order::Voicemail),
            ("accept", None) => Ok(Order::Accept),
            ("accept", Some("end")) => Ok(Order::AcceptEnding),
            ("end", Some("all")) => Ok(Order::EndAll),
            ("end", id) => Ok(Order::End(id.map(str::to_string))),
            ("hold", w) => on_off(w).map(Order::Hold),
            ("mute", w) => on_off(w).map(Order::Mute),
            ("swap", None) => Ok(Order::Swap),
            _ => Err(format!("unknown call order {verb:?}")),
        }
    }

    /// The order as `parse` reads it
    pub fn text(&self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        match self {
            Order::Dial(n) => format!("dial {n}"),
            Order::Redial => "redial".into(),
            Order::Voicemail => "voicemail".into(),
            Order::Accept => "accept".into(),
            Order::AcceptEnding => "accept end".into(),
            Order::End(Some(id)) => format!("end {id}"),
            Order::End(None) => "end".into(),
            Order::EndAll => "end all".into(),
            Order::Hold(b) => format!("hold {}", on(*b)),
            Order::Mute(b) => format!("mute {}", on(*b)),
            Order::Swap => "swap".into(),
        }
    }

    /// For the log: what, without the number
    pub fn what(&self) -> &'static str {
        match self {
            Order::Dial(_) => "dial",
            Order::Redial => "redial",
            Order::Voicemail => "voicemail",
            Order::Accept | Order::AcceptEnding => "accept",
            Order::End(_) | Order::EndAll => "end",
            Order::Hold(_) => "hold",
            Order::Mute(_) => "mute",
            Order::Swap => "swap",
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let call = |call_type, destination_id, service| InitiateCall {
            call_type,
            destination_id,
            service,
            address_book_id: None,
        };
        match self {
            Order::Dial(n) => call(0, Some(n.clone()), Some(1)).encode(),
            Order::Voicemail => call(1, None, None).encode(),
            Order::Redial => call(2, None, None).encode(),
            Order::Accept => AcceptCall {
                action: 0,
                call_uuid: None,
            }
            .encode(),
            Order::AcceptEnding => AcceptCall {
                action: 1,
                call_uuid: None,
            }
            .encode(),
            Order::End(id) => EndCall {
                action: 0,
                call_uuid: id.clone(),
            }
            .encode(),
            Order::EndAll => EndCall {
                action: 1,
                call_uuid: None,
            }
            .encode(),
            Order::Hold(on) => HoldStatusUpdate {
                hold_status: *on,
                call_uuid: None,
            }
            .encode(),
            Order::Mute(on) => MuteStatusUpdate { mute_status: *on }.encode(),
            Order::Swap => SwapCalls {}.encode(),
        }
    }
}

/// The subscription to the lists, byte for byte the request a car got an answer to (iap2-csm tests/calls.rs): every
/// field of an entry, at most 25 recents with calls in a row put together, the favourites
pub fn start_lists() -> StartListUpdates {
    StartListUpdates {
        recents_available: false,
        recents_properties: Some(RecentsListProperties {
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
        }),
        recents_count: false,
        recents_max: Some(25),
        recents_combine: Some(true),
        favorites_available: false,
        favorites_properties: Some(FavoritesListProperties {
            index: true,
            remote_id: true,
            display_name: true,
            label: true,
            address_book_id: true,
            service: true,
        }),
        favorites_count: false,
        favorites_max: Some(0),
    }
}

/// For the core when the phone refused the call messages of a session that wanted them: no calls through it
pub fn refused_json() -> String {
    json!({ "type": "callLists", "refused": true, "recents": [], "favorites": [] }).to_string()
}

/// The lists as the phone last sent them, put together from its messages: a count sets the length, an entry lands at
/// its index
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Lists {
    recents: Vec<Option<ListEntry>>,
    favorites: Vec<Option<ListEntry>>,
}

fn merge(
    list: &mut Vec<Option<ListEntry>>,
    available: Option<bool>,
    count: Option<u16>,
    entries: Vec<ListEntry>,
) {
    if available == Some(false) {
        list.clear();
    }
    if let Some(n) = count {
        list.resize(usize::from(n).min(LIST_MAX), None);
    }
    for e in entries {
        let Some(at) = e.index.map(usize::from).filter(|at| *at < LIST_MAX) else {
            continue;
        };
        if list.len() <= at {
            list.resize(at + 1, None);
        }
        list[at] = Some(e);
    }
}

fn kind(e: &ListEntry) -> &'static str {
    match e.kind {
        Some(1) => "in",
        Some(2) => "out",
        Some(3) => "missed",
        _ => "",
    }
}

fn entry_json(e: &ListEntry, recent: bool) -> Value {
    let number = e.remote_id.clone().unwrap_or_default();
    // without a contact the phone sends the number written out as the name; a name that is no number stays, also
    // beside an address that is none (FaceTime with an e-mail address, no number at all)
    let name = e
        .display_name
        .clone()
        .filter(|n| dialable(n).is_none() || dialable(n) != dialable(&number))
        .unwrap_or_default();
    let mut o = json!({
        "number": number,
        "name": name,
        "label": e.label.clone().unwrap_or_default(),
    });
    if recent {
        o["kind"] = json!(kind(e));
        o["time"] = json!(e.unix_timestamp.unwrap_or(0));
        o["count"] = json!(e.occurrences.unwrap_or(1));
    }
    o
}

impl Lists {
    pub fn apply(&mut self, m: ListUpdate) {
        merge(
            &mut self.recents,
            m.recents_available,
            m.recents_count,
            m.recents,
        );
        merge(
            &mut self.favorites,
            m.favorites_available,
            m.favorites_count,
            m.favorites,
        );
    }

    /// `{"type":"callLists","recents":[...],"favorites":[...]}`, entries the phone has not sent yet left out
    pub fn json(&self) -> String {
        let list = |l: &[Option<ListEntry>], recent| {
            l.iter()
                .flatten()
                .map(|e| entry_json(e, recent))
                .collect::<Vec<_>>()
        };
        json!({
            "type": "callLists",
            "recents": list(&self.recents, true),
            "favorites": list(&self.favorites, false),
        })
        .to_string()
    }
}

/// An order for the session of one phone (its Bluetooth address), or of any
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Targeted {
    pub order: Order,
    pub bt_mac: Option<String>,
}

impl Targeted {
    /// `<bluetooth address>|- <order>`, as the helper's socket takes it after `call`
    pub fn parse(arg: &str) -> Result<Self, String> {
        let (who, order) = arg
            .trim()
            .split_once(' ')
            .ok_or("call <address>|- <order>")?;
        Ok(Self {
            order: Order::parse(order)?,
            bt_mac: (who != "-").then(|| who.to_string()),
        })
    }

    pub fn meant_for(&self, phone_bt_mac: Option<&str>) -> bool {
        match (self.bt_mac.as_deref(), phone_bt_mac) {
            (Some(target), Some(phone)) => target.eq_ignore_ascii_case(phone),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_masks_every_text_and_keeps_the_numbers() {
        let update = CallStateUpdate {
            remote_id: Some("+41790000001".into()),
            display_name: Some("Test Eins".into()),
            status: Some(4),
            direction: Some(2),
            call_uuid: Some("7D19170A-959C".into()),
            disconnect_reason: None,
        };
        let line = describe(&update.encode()).unwrap();
        assert_eq!(
            line,
            "0x4155: 0 text<12…01> | 1 text<9> | 2 u8 4 | 3 u8 2 | 4 text<13…59>"
        );
        assert!(!line.contains("Test") && !line.contains("790000"));
        let lists = ListUpdate {
            recents: vec![ListEntry {
                index: Some(0),
                remote_id: Some("+41790000001".into()),
                display_name: Some("Test".into()),
                label: None,
                address_book_id: None,
                service: Some(1),
                kind: Some(3),
                unix_timestamp: Some(1_790_000_000),
                duration: None,
                occurrences: None,
            }],
            ..Default::default()
        };
        assert_eq!(
            describe(&lists.encode()).unwrap(),
            "0x4171: 1 {0 u16 0, 1 text<12…01>, 2 text<4>, 5 u8 1, 6 u8 3, 7 u64 1790000000}"
        );
        assert_eq!(
            describe(&[0x40, 0x40, 0x00, 0x06, 0x50, 0x01]),
            None,
            "not a call message"
        );
        // a text with a control character, texts without their 0x00, a group deeper than the log goes
        assert_eq!(value(b"Ann\ta B\0", 0), "text<7>");
        assert_eq!(value(b"Anna", 0), "bytes<4>");
        assert_eq!(value(b"Anna Bea", 0), "bytes<8>");
        assert_eq!(value(b"Al", 0), "bytes<2>");
        let mut deep = b"Test\0".to_vec();
        for id in 0..5u16 {
            let mut outer = Vec::new();
            iap2_csm::put_param(&mut outer, id, &deep);
            deep = outer;
        }
        let line = value(&deep, 0);
        assert!(!line.contains("Test") && line.contains("bytes<"), "{line}");
    }

    #[test]
    fn the_full_subscriptions_ask_for_every_field() {
        let (calls, comms) = full_subscriptions();
        let m = StartCallStateUpdates::decode(&calls).unwrap();
        assert!(m.remote_id && m.address_book_id && m.label && m.service && m.start_timestamp);
        assert_eq!(describe(&comms).unwrap().matches("flag").count(), 18);
    }

    #[test]
    fn orders_read_back_as_written() {
        for text in [
            "dial +41790000001",
            "redial",
            "voicemail",
            "accept",
            "accept end",
            "end",
            "end 7D19170A-959C",
            "end all",
            "hold on",
            "hold off",
            "mute on",
            "mute off",
            "swap",
        ] {
            assert_eq!(Order::parse(text).unwrap().text(), text);
        }
    }

    #[test]
    fn numbers_lose_what_one_writes_between_digits() {
        assert_eq!(
            Order::parse("dial +41 (79) 000-00.01").unwrap(),
            Order::Dial("+41790000001".into())
        );
        assert_eq!(dialable("*31#079"), Some("*31#079".into()));
        for wrong in ["", "abc", "+", "07+9", "1;rm", &"1".repeat(33)] {
            assert_eq!(dialable(wrong), None, "{wrong:?}");
        }
        assert!(Order::parse("dial").is_err());
        assert!(Order::parse("mute").is_err());
        assert!(Order::parse("mute maybe").is_err());
        assert!(Order::parse("join").is_err());
    }

    #[test]
    fn orders_become_the_messages_of_call_control() {
        let id = |o: Order| u16::from_be_bytes([o.encode()[4], o.encode()[5]]);
        assert_eq!(id(Order::Dial("1".into())), 0x415A);
        assert_eq!(id(Order::Accept), 0x415B);
        assert_eq!(id(Order::End(None)), 0x415C);
        assert_eq!(id(Order::Swap), 0x415D);
        assert_eq!(id(Order::Hold(true)), 0x415F);
        assert_eq!(id(Order::Mute(true)), 0x4160);
        let dial = InitiateCall::decode(&Order::Dial("+4179".into()).encode()).unwrap();
        assert_eq!(
            (dial.call_type, dial.destination_id.as_deref(), dial.service),
            (0, Some("+4179"), Some(1))
        );
        let redial = InitiateCall::decode(&Order::Redial.encode()).unwrap();
        assert_eq!((redial.call_type, redial.destination_id), (2, None));
        let reject = EndCall::decode(&Order::End(Some("u-1".into())).encode()).unwrap();
        assert_eq!(
            (reject.action, reject.call_uuid.as_deref()),
            (0, Some("u-1"))
        );
        assert_eq!(EndCall::decode(&Order::EndAll.encode()).unwrap().action, 1);
        assert_eq!(
            AcceptCall::decode(&Order::AcceptEnding.encode())
                .unwrap()
                .action,
            1
        );
    }

    #[test]
    fn every_message_sent_is_named_in_the_identification() {
        for o in [
            Order::Dial("1".into()),
            Order::Accept,
            Order::End(None),
            Order::Swap,
            Order::Hold(false),
            Order::Mute(false),
        ] {
            let msg = u16::from_be_bytes([o.encode()[4], o.encode()[5]]);
            assert!(SENT.contains(&msg), "{msg:04X}");
        }
        let start = start_lists().encode();
        assert!(SENT.contains(&u16::from_be_bytes([start[4], start[5]])));
    }

    #[test]
    fn the_subscription_is_the_request_a_car_got_an_answer_to() {
        // iap2-csm tests/calls.rs, "verified_start" of ProManiFest2122/mhi2-k3346-carplay-vc
        let verified: &[u8] = &[
            0x00, 0x2c, 0x00, 0x01, //
            0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0x02, 0x00, 0x04,
            0x00, 0x03, 0x00, 0x04, 0x00, 0x04, 0x00, 0x04, 0x00, 0x05, 0x00, 0x04, 0x00, 0x06,
            0x00, 0x04, 0x00, 0x07, 0x00, 0x04, 0x00, 0x08, 0x00, 0x04, 0x00, 0x09, //
            0x00, 0x06, 0x00, 0x03, 0x00, 0x19, //
            0x00, 0x05, 0x00, 0x04, 0x01, //
            0x00, 0x1c, 0x00, 0x06, //
            0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x04, 0x00, 0x02, 0x00, 0x04,
            0x00, 0x03, 0x00, 0x04, 0x00, 0x04, 0x00, 0x04, 0x00, 0x05, //
            0x00, 0x06, 0x00, 0x08, 0x00, 0x00,
        ];
        let mut frame = vec![0x40, 0x40];
        frame.extend_from_slice(&(verified.len() as u16 + 6).to_be_bytes());
        frame.extend_from_slice(&0x4170u16.to_be_bytes());
        frame.extend_from_slice(verified);
        assert_eq!(start_lists().encode(), frame);
    }

    #[test]
    fn a_name_stays_beside_an_address_that_is_no_number() {
        let mut facetime = entry(0, "anna@example.org", "Anna", 1);
        let json = entry_json(&facetime, true);
        assert_eq!(
            (json["name"].as_str(), json["number"].as_str()),
            (Some("Anna"), Some("anna@example.org"))
        );
        facetime.remote_id = None;
        assert_eq!(entry_json(&facetime, true)["name"], "Anna");
        let written_out = entry(0, "+41790000002", "+41 79 000 00 02", 3);
        assert_eq!(entry_json(&written_out, true)["name"], "");
        let other_number = entry(0, "+41790000002", "+41 79 000 00 09", 3);
        assert_eq!(entry_json(&other_number, true)["name"], "+41 79 000 00 09");
    }

    #[test]
    fn a_refused_order_says_its_verb_and_nothing_of_a_number() {
        for bad in ["call 0791234567", "mute 0791234567", "hold 0791234567"] {
            let e = Order::parse(bad).unwrap_err();
            assert!(!e.contains("079"), "{e}");
        }
    }

    fn entry(index: u16, number: &str, name: &str, kind: u8) -> ListEntry {
        ListEntry {
            index: Some(index),
            remote_id: Some(number.into()),
            display_name: Some(name.into()),
            label: Some("mobile".into()),
            address_book_id: None,
            service: Some(1),
            kind: Some(kind),
            unix_timestamp: Some(1_790_000_000),
            duration: None,
            occurrences: Some(2),
        }
    }

    #[test]
    fn lists_come_together_from_their_parts() {
        let mut lists = Lists::default();
        lists.apply(ListUpdate {
            recents_available: Some(true),
            recents_count: Some(2),
            ..Default::default()
        });
        lists.apply(ListUpdate {
            recents: vec![entry(1, "+41790000002", "+41 79 000 00 02", 3)],
            ..Default::default()
        });
        let v: Value = serde_json::from_str(&lists.json()).unwrap();
        assert_eq!(v["type"], "callLists");
        assert_eq!(
            v["recents"].as_array().unwrap().len(),
            1,
            "entry 0 has not come yet"
        );
        lists.apply(ListUpdate {
            recents: vec![entry(0, "+41790000001", "Test Eins", 2)],
            favorites_available: Some(true),
            favorites_count: Some(1),
            favorites: vec![entry(0, "+41790000003", "Test Drei", 0)],
            ..Default::default()
        });
        let v: Value = serde_json::from_str(&lists.json()).unwrap();
        let recents = v["recents"].as_array().unwrap();
        assert_eq!(recents[0]["name"], "Test Eins");
        assert_eq!(recents[0]["kind"], "out");
        assert_eq!(recents[0]["count"], 2);
        assert_eq!(recents[0]["time"], 1_790_000_000u64);
        assert_eq!(recents[1]["name"], "", "a number written out is no name");
        assert_eq!(recents[1]["kind"], "missed");
        assert_eq!(v["favorites"][0]["number"], "+41790000003");
        assert!(v["favorites"][0].get("kind").is_none());
    }

    #[test]
    fn a_shorter_list_and_a_list_gone_shrink_it() {
        let mut lists = Lists::default();
        lists.apply(ListUpdate {
            recents_count: Some(2),
            recents: vec![entry(0, "1", "A", 1), entry(1, "2", "B", 1)],
            ..Default::default()
        });
        lists.apply(ListUpdate {
            recents_count: Some(1),
            ..Default::default()
        });
        let v: Value = serde_json::from_str(&lists.json()).unwrap();
        assert_eq!(v["recents"].as_array().unwrap().len(), 1);
        lists.apply(ListUpdate {
            recents_available: Some(false),
            recents_count: Some(0),
            ..Default::default()
        });
        let v: Value = serde_json::from_str(&lists.json()).unwrap();
        assert!(v["recents"].as_array().unwrap().is_empty());
    }

    #[test]
    fn an_index_far_out_is_dropped() {
        let mut lists = Lists::default();
        lists.apply(ListUpdate {
            recents: vec![entry(60_000, "1", "A", 1)],
            ..Default::default()
        });
        assert_eq!(lists, Lists::default());
    }

    #[test]
    fn orders_name_their_phone_or_any() {
        let t = Targeted::parse("AA:BB:CC:DD:EE:FF dial 0790000001").unwrap();
        assert_eq!(t.order, Order::Dial("0790000001".into()));
        assert!(t.meant_for(Some("aa:bb:cc:dd:ee:ff")));
        assert!(!t.meant_for(Some("11:22:33:44:55:66")));
        assert!(t.meant_for(None));
        let any = Targeted::parse("- end all").unwrap();
        assert_eq!((any.order, any.bt_mac), (Order::EndAll, None));
        assert!(Targeted::parse("end").is_err());
        assert!(Targeted::parse("- dial nothing").is_err());
    }
}
