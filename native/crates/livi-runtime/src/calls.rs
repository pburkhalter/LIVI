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
        other => Err(format!("on or off, not {other:?}")),
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
            _ => Err(format!("unknown call order {text:?}")),
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

/// The subscription to the lists: every field, one recent entry per number, as many as the phone gives
pub fn start_lists() -> StartListUpdates {
    StartListUpdates {
        recents_available: true,
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
        recents_count: true,
        recents_max: Some(0),
        recents_combine: Some(true),
        favorites_available: true,
        favorites_properties: Some(FavoritesListProperties {
            index: true,
            remote_id: true,
            display_name: true,
            label: true,
            address_book_id: true,
            service: true,
        }),
        favorites_count: true,
        favorites_max: Some(0),
    }
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
    // without a contact the phone sends the number written out as the name
    let name = e
        .display_name
        .clone()
        .filter(|n| dialable(n).as_deref() != dialable(&number).as_deref())
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
    fn the_subscription_asks_for_every_field() {
        let m = StartListUpdates::decode(&start_lists().encode()).unwrap();
        assert!(m.recents_properties.unwrap().occurrences);
        assert!(m.favorites_properties.unwrap().service);
        assert_eq!(
            (m.recents_max, m.favorites_max, m.recents_combine),
            (Some(0), Some(0), Some(true))
        );
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
