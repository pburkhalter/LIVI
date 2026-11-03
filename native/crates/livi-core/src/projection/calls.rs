//! Calls through CarPlay for UI plugins (LIVI_CP_CALLS=1, livi_runtime::calls on the helper's side): the call state
//! and the lists the phone sends become `State::call`; `Action::Call` goes to the phone as iAP2 call control, the keys
//! of the keypad as CarPlay telephony keys.

use super::*;
use livi_core_proto::state::{Call, CallEntry};

pub const ENV: &str = "LIVI_CP_CALLS";

pub fn enabled() -> bool {
    std::env::var(ENV).is_ok_and(|v| v == "1")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// One call the phone told of, by its id
#[derive(Debug, Clone, PartialEq)]
struct Line {
    id: String,
    /// The phone's: 1 sending, 2 ringing, 3 connecting, 4 active, 5 held, 6 disconnecting
    status: u8,
    incoming: bool,
    number: String,
    name: String,
    since: u64,
}

impl Line {
    fn state(&self) -> &'static str {
        match self.status {
            1 => "dialing",
            2 | 3 if self.incoming => "ringing",
            2 | 3 => "dialing",
            4 => "active",
            5 => "held",
            6 => "ending",
            _ => "idle",
        }
    }

    /// Which call the page shows when there are several: the one in conversation first, a ringing one last
    fn rank(&self) -> u8 {
        match self.state() {
            "active" => 0,
            "dialing" => 1,
            "held" => 2,
            "ringing" => 3,
            _ => 4,
        }
    }

    fn who(&self) -> &str {
        if self.name.is_empty() {
            &self.number
        } else {
            &self.name
        }
    }
}

/// The calls and the lists of one phone
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Calls {
    lines: Vec<Line>,
    muted: bool,
    recents: Vec<CallEntry>,
    favorites: Vec<CallEntry>,
}

fn entries(list: &Value) -> Vec<CallEntry> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let text = |k: &str| {
                e.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let num = |k: &str| e.get(k).and_then(Value::as_u64).unwrap_or(0);
            CallEntry {
                number: text("number"),
                name: text("name"),
                label: text("label"),
                kind: text("kind"),
                time: num("time"),
                count: num("count") as u32,
            }
        })
        .collect()
}

impl Calls {
    /// An event of the helper, `call` or `callLists`; whether anything changed. A helper without calls tells only
    /// the phase (ringing, active, ended), number and name
    pub(super) fn take(&mut self, event: &Value, now: u64) -> bool {
        let before = self.clone();
        let text = |k: &str| event.get(k).and_then(Value::as_str).map(str::to_string);
        match event.get("type").and_then(Value::as_str) {
            Some("callLists") => {
                self.recents = entries(&event["recents"]);
                self.recents.sort_by_key(|e| std::cmp::Reverse(e.time));
                self.favorites = entries(&event["favorites"]);
            }
            Some("call") => {
                let phase = text("phase").unwrap_or_default();
                let status = event.get("status").and_then(Value::as_u64).map_or(
                    match phase.as_str() {
                        "ringing" => 2,
                        "active" => 4,
                        _ => 0,
                    },
                    |s| s as u8,
                );
                let direction = event.get("direction").and_then(Value::as_u64);
                let id = text("uuid").unwrap_or_default();
                if status == 0 {
                    self.lines.retain(|l| l.id != id);
                } else {
                    let at = match self.lines.iter().position(|l| l.id == id) {
                        Some(at) => at,
                        None => {
                            self.lines.push(Line {
                                id,
                                status,
                                // without a direction: a call that first rings came in
                                incoming: direction.map_or(status == 2, |d| d == 1),
                                number: String::new(),
                                name: String::new(),
                                since: 0,
                            });
                            self.lines.len() - 1
                        }
                    };
                    let line = &mut self.lines[at];
                    line.status = status;
                    if let Some(d) = direction {
                        line.incoming = d == 1;
                    }
                    if let Some(n) = text("number").filter(|n| !n.is_empty()) {
                        line.number = n;
                    }
                    if let Some(n) = text("name") {
                        line.name = n;
                    }
                    if status == 4 && line.since == 0 {
                        line.since = now;
                    }
                }
                if self.lines.is_empty() {
                    self.muted = false;
                }
            }
            _ => return false,
        }
        *self != before
    }

    /// What the page sees
    pub(super) fn view(&self) -> Call {
        let front = self.lines.iter().min_by_key(|l| l.rank());
        let waiting = self
            .lines
            .iter()
            .filter(|l| l.state() == "ringing" && Some(*l) != front)
            .map(|l| l.who().to_string())
            .next()
            .unwrap_or_default();
        Call {
            available: true,
            state: front.map_or("idle", Line::state).into(),
            incoming: front.is_some_and(|l| l.incoming),
            number: front.map(|l| l.number.clone()).unwrap_or_default(),
            name: front.map(|l| l.name.clone()).unwrap_or_default(),
            id: front.map(|l| l.id.clone()).unwrap_or_default(),
            since: front.map_or(0, |l| l.since),
            muted: self.muted,
            waiting,
            recents: self.recents.clone(),
            favorites: self.favorites.clone(),
        }
    }

    /// The id of the call that rings, for "reject": ending the frontmost call would end the one in conversation
    fn ringing(&self) -> Option<&str> {
        self.lines
            .iter()
            .find(|l| l.state() == "ringing")
            .map(|l| l.id.as_str())
            .filter(|id| !id.is_empty())
    }
}

/// A key of the keypad as a CarPlay telephony key
fn tone(key: &str) -> Option<u8> {
    use telephony_button as t;
    match key {
        "*" => Some(t::STAR),
        "#" => Some(t::POUND),
        d if d.len() == 1 => d.parse::<u8>().ok().map(|d| t::KEY0 + d),
        _ => None,
    }
}

/// What goes to the helper for an order of a plugin (Action::Call); `ringing` is the id of a call that rings
fn order_for_helper(order: &str, ringing: Option<&str>) -> String {
    match order {
        "reject" => match ringing {
            Some(id) => format!("end {id}"),
            None => "end".into(),
        },
        other => other.to_string(),
    }
}

impl Projection {
    /// A call event of a CarPlay session (Metadata `call`, `callLists`)
    pub(super) fn call_event(&mut self, link: Link, event: &Value) {
        let Some(entry) = self.by_link(link) else {
            return;
        };
        let changed = entry.calls.take(event, unix_now());
        if changed && self.is_active(link) {
            self.publish();
        }
    }

    /// The call state of the active phone for the state, empty without calls or without CarPlay
    pub(super) fn call_view(&self) -> Call {
        match self.active() {
            Some(s) if enabled() && matches!(s.link, Link::Cp(_)) => s.calls.view(),
            _ => Call::default(),
        }
    }

    /// An order of a plugin (Action::Call)
    pub(super) fn call(&mut self, order: &str) {
        let order = order.trim();
        let Some((id, bt_mac, ringing)) = self.active().and_then(|s| match s.link {
            Link::Cp(id) => Some((
                id,
                s.device.bt_mac.clone(),
                s.calls.ringing().map(str::to_string),
            )),
            Link::Aa(_) => None,
        }) else {
            println!("[core] call: no CarPlay session");
            return;
        };
        if let Some(key) = order.strip_prefix("dtmf ") {
            match tone(key.trim()) {
                Some(button) => self.cp.stack(id, StackCmd::Telephony(button)),
                None => println!("[core] call: no key {key:?} on a keypad"),
            }
            return;
        }
        let mute = match order {
            "mute on" => Some(true),
            "mute off" => Some(false),
            _ => None,
        };
        if let Some(on) = mute
            && let Some(entry) = self.sessions.iter_mut().find(|s| s.link == Link::Cp(id))
        {
            entry.calls.muted = on && !entry.calls.lines.is_empty();
            self.publish();
        }
        self.devices
            .call(order_for_helper(order, ringing.as_deref()), bt_mac);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(status: u64, direction: u64, id: &str) -> Value {
        json!({ "type": "call", "phase": "active", "status": status, "direction": direction, "uuid": id,
                "number": "+41790000001", "name": "Test Eins" })
    }

    #[test]
    fn a_call_goes_from_ringing_to_active_to_gone() {
        let mut c = Calls::default();
        assert!(c.take(&call(2, 1, "a"), 100));
        let v = c.view();
        assert_eq!(
            (v.state.as_str(), v.incoming, v.since),
            ("ringing", true, 0)
        );
        assert_eq!(
            (v.number.as_str(), v.name.as_str(), v.id.as_str()),
            ("+41790000001", "Test Eins", "a")
        );
        assert!(c.take(&call(4, 1, "a"), 105));
        assert_eq!((c.view().state.as_str(), c.view().since), ("active", 105));
        assert!(
            !c.take(&call(4, 1, "a"), 110),
            "the same again changes nothing, not even the start"
        );
        c.muted = true;
        assert!(c.take(
            &json!({ "type": "call", "phase": "ended", "status": 0, "uuid": "a" }),
            200
        ));
        let v = c.view();
        assert_eq!(
            (v.state.as_str(), v.number.as_str(), v.muted),
            ("idle", "", false)
        );
        assert!(v.available);
    }

    #[test]
    fn an_outgoing_call_dials_until_it_is_answered() {
        let mut c = Calls::default();
        c.take(&call(1, 2, "b"), 0);
        assert_eq!(c.view().state, "dialing");
        c.take(&call(3, 2, "b"), 0);
        assert_eq!(
            (c.view().state.as_str(), c.view().incoming),
            ("dialing", false)
        );
        c.take(&call(5, 2, "b"), 0);
        assert_eq!(c.view().state, "held");
    }

    #[test]
    fn a_second_call_waits_beside_the_one_in_conversation() {
        let mut c = Calls::default();
        c.take(&call(4, 2, "a"), 1);
        let mut second = call(2, 1, "b");
        second["name"] = json!("");
        second["number"] = json!("+41790000002");
        c.take(&second, 2);
        let v = c.view();
        assert_eq!((v.id.as_str(), v.state.as_str()), ("a", "active"));
        assert_eq!(v.waiting, "+41790000002");
        assert_eq!(c.ringing(), Some("b"));
        assert_eq!(order_for_helper("reject", c.ringing()), "end b");
        assert_eq!(order_for_helper("end", c.ringing()), "end");
    }

    #[test]
    fn an_older_helper_tells_only_the_phase() {
        let mut c = Calls::default();
        c.take(
            &json!({ "type": "call", "phase": "ringing", "number": "+41790000001" }),
            0,
        );
        assert_eq!(
            (c.view().state.as_str(), c.view().incoming),
            ("ringing", true)
        );
        assert_eq!(c.ringing(), None, "no id: reject ends the frontmost call");
        c.take(&json!({ "type": "call", "phase": "ended" }), 0);
        assert_eq!(c.view().state, "idle");
    }

    #[test]
    fn lists_come_newest_first() {
        let mut c = Calls::default();
        let lists = json!({ "type": "callLists",
            "recents": [
                { "number": "1", "name": "A", "label": "mobile", "kind": "in", "time": 10, "count": 1 },
                { "number": "2", "name": "", "label": "", "kind": "missed", "time": 30, "count": 2 }
            ],
            "favorites": [ { "number": "3", "name": "C", "label": "home" } ] });
        assert!(c.take(&lists, 0));
        let v = c.view();
        assert_eq!(v.recents[0].number, "2");
        assert_eq!(
            (v.recents[0].kind.as_str(), v.recents[0].count),
            ("missed", 2)
        );
        assert_eq!(v.favorites[0].label, "home");
        assert!(!c.take(&lists, 0));
        assert!(!c.take(&json!({ "type": "nowplaying" }), 0));
    }

    #[test]
    fn keypad_keys_are_telephony_keys() {
        use telephony_button as t;
        assert_eq!(tone("0"), Some(t::KEY0));
        assert_eq!(tone("9"), Some(t::KEY0 + 9));
        assert_eq!((tone("*"), tone("#")), (Some(t::STAR), Some(t::POUND)));
        assert_eq!((tone("12"), tone("a"), tone("")), (None, None, None));
    }
}
