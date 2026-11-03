//! Calls through CarPlay for UI plugins (LIVI_CP_CALLS=1, livi_runtime::calls on the helper's side): the call state
//! and the lists the phone sends become `State::call`; `Action::Call` goes to the phone as iAP2 call control, the keys
//! of the keypad as CarPlay telephony keys.

use super::*;
use livi_core_proto::state::{Call, CallEntry};

pub const ENV: &str = "LIVI_CP_CALLS";

pub fn enabled() -> bool {
    std::env::var(ENV).is_ok_and(|v| v == "1")
}

/// Seconds "ending" stays after the last call, then "idle"
pub(super) const ENDING_SECS: u64 = 2;

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
    /// When anything last changed (Unix seconds), for `elapsed`
    stamp: u64,
    /// When the last call ended, for "ending"
    ended: Option<u64>,
    /// The time of the newest recent call the page saw (Action::Call "seen"), for `missed`
    seen: u64,
}

/// Up to two letters of a name: of its first and its last word that begin with a letter, as t100-phone does
fn initials(name: &str) -> String {
    let letters: Vec<char> = name
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter_map(|w| w.chars().next())
        .filter(|c| c.is_alphabetic())
        .collect();
    match letters.as_slice() {
        [] => String::new(),
        [one] => one.to_uppercase().collect(),
        [first, .., last] => first.to_uppercase().chain(last.to_uppercase()).collect(),
    }
}

/// A local date and time: year, month, day, hour, minute
type Local = (i32, i32, i32, i32, i32);

fn local(t: u64) -> Local {
    let t = t as libc::time_t;
    // SAFETY: an all-zero tm is valid, localtime_r only writes into it
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return (0, 0, 0, 0, 0);
    }
    (
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
    )
}

/// A recent call's time for the page: the time today, "yesterday" in LIVI's language, else the date
fn when_text(
    then: Local,
    today: Local,
    yesterday: Local,
    lang: crate::nav_text::Language,
) -> String {
    use crate::nav_text::Language;
    let day = |l: Local| (l.0, l.1, l.2);
    if day(then) == day(today) {
        format!("{:02}:{:02}", then.3, then.4)
    } else if day(then) == day(yesterday) {
        match lang {
            Language::En => "Yesterday",
            Language::De => "Gestern",
            Language::Fr => "Hier",
            Language::Ua => "Вчора",
        }
        .into()
    } else if lang == Language::En {
        format!("{:02}/{:02}", then.2, then.1)
    } else {
        format!("{:02}.{:02}.", then.2, then.1)
    }
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
                when: String::new(),
                initials: String::new(),
            }
        })
        .collect()
}

impl Calls {
    /// An event of the helper, `call` or `callLists`; whether anything changed. A helper without calls tells only
    /// the phase (ringing, active, ended), number and name
    pub(super) fn take(&mut self, event: &Value, now: u64) -> bool {
        let before = self.clone();
        let had_calls = !self.lines.is_empty();
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
                    if had_calls {
                        self.ended = Some(now);
                    }
                } else {
                    self.ended = None;
                }
            }
            _ => return false,
        }
        let changed = *self != before;
        if changed {
            self.stamp = now;
        }
        changed
    }

    /// The page showed the missed calls
    pub(super) fn seen(&mut self) {
        self.seen = self.recents.iter().map(|e| e.time).max().unwrap_or(0);
    }

    /// Whether "ending" shows at this time
    pub(super) fn ending(&self, now: u64) -> bool {
        self.lines.is_empty() && self.ended.is_some_and(|t| now < t + ENDING_SECS)
    }

    /// What the page sees at this time, the recents' times in this language
    pub(super) fn view(&self, now: u64, lang: crate::nav_text::Language) -> Call {
        let front = self.lines.iter().min_by_key(|l| l.rank());
        let (today, yesterday) = (local(now), local(now.saturating_sub(86_400)));
        let written = |e: &CallEntry, recent: bool| CallEntry {
            when: if recent {
                when_text(local(e.time), today, yesterday, lang)
            } else {
                String::new()
            },
            initials: initials(&e.name),
            ..e.clone()
        };
        let missed = self
            .recents
            .iter()
            .take_while(|e| e.kind == "missed" && e.time > self.seen)
            .map(|e| e.count.max(1))
            .sum();
        let state = match front {
            Some(l) => l.state(),
            None if self.ending(now) => "ending",
            None => "idle",
        };
        let waiting = self
            .lines
            .iter()
            .filter(|l| l.state() == "ringing" && Some(*l) != front)
            .map(|l| l.who().to_string())
            .next()
            .unwrap_or_default();
        Call {
            available: true,
            state: state.into(),
            incoming: front.is_some_and(|l| l.incoming),
            number: front.map(|l| l.number.clone()).unwrap_or_default(),
            name: front.map(|l| l.name.clone()).unwrap_or_default(),
            id: front.map(|l| l.id.clone()).unwrap_or_default(),
            since: front.map_or(0, |l| l.since),
            elapsed: front
                .filter(|l| l.since > 0)
                .map_or(0, |l| self.stamp.saturating_sub(l.since)),
            muted: self.muted,
            waiting,
            missed,
            recents: self.recents.iter().map(|e| written(e, true)).collect(),
            favorites: self.favorites.iter().map(|e| written(e, false)).collect(),
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
        if !enabled() {
            return;
        }
        let Some(entry) = self.by_link(link) else {
            return;
        };
        let now = unix_now();
        let changed = entry.calls.take(event, now);
        let ending = entry.calls.ending(now);
        if ending {
            // "ending" shows for a moment, then "idle": one more publication then
            self.call_ending = Some(tokio::time::Instant::now() + Duration::from_secs(ENDING_SECS));
        }
        if changed && self.is_active(link) {
            self.publish();
        }
    }

    /// The call state of the active phone for the state, empty without calls or without CarPlay
    pub(super) fn call_view(&self) -> Call {
        match self.active() {
            Some(s) if enabled() && matches!(s.link, Link::Cp(_)) => s.calls.view(
                unix_now(),
                crate::nav_text::Language::of(&self.config_seen.language),
            ),
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
        if order == "seen" {
            if let Some(entry) = self.sessions.iter_mut().find(|s| s.link == Link::Cp(id)) {
                entry.calls.seen();
            }
            self.publish();
            return;
        }
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
    use crate::nav_text::Language;
    use serde_json::json;

    fn call(status: u64, direction: u64, id: &str) -> Value {
        json!({ "type": "call", "phase": "active", "status": status, "direction": direction, "uuid": id,
                "number": "+41790000001", "name": "Test Eins" })
    }

    #[test]
    fn a_call_goes_from_ringing_to_active_to_gone() {
        let mut c = Calls::default();
        assert!(c.take(&call(2, 1, "a"), 100));
        let v = c.view(1_000, Language::De);
        assert_eq!(
            (v.state.as_str(), v.incoming, v.since),
            ("ringing", true, 0)
        );
        assert_eq!(
            (v.number.as_str(), v.name.as_str(), v.id.as_str()),
            ("+41790000001", "Test Eins", "a")
        );
        assert!(c.take(&call(4, 1, "a"), 105));
        assert_eq!(
            (
                c.view(1_000, Language::De).state.as_str(),
                c.view(1_000, Language::De).since
            ),
            ("active", 105)
        );
        assert!(
            !c.take(&call(4, 1, "a"), 110),
            "the same again changes nothing, not even the start"
        );
        c.muted = true;
        assert!(c.take(
            &json!({ "type": "call", "phase": "ended", "status": 0, "uuid": "a" }),
            200
        ));
        let v = c.view(1_000, Language::De);
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
        assert_eq!(c.view(1_000, Language::De).state, "dialing");
        c.take(&call(3, 2, "b"), 0);
        assert_eq!(
            (
                c.view(1_000, Language::De).state.as_str(),
                c.view(1_000, Language::De).incoming
            ),
            ("dialing", false)
        );
        c.take(&call(5, 2, "b"), 0);
        assert_eq!(c.view(1_000, Language::De).state, "held");
    }

    #[test]
    fn a_second_call_waits_beside_the_one_in_conversation() {
        let mut c = Calls::default();
        c.take(&call(4, 2, "a"), 1);
        let mut second = call(2, 1, "b");
        second["name"] = json!("");
        second["number"] = json!("+41790000002");
        c.take(&second, 2);
        let v = c.view(1_000, Language::De);
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
            (
                c.view(1_000, Language::De).state.as_str(),
                c.view(1_000, Language::De).incoming
            ),
            ("ringing", true)
        );
        assert_eq!(c.ringing(), None, "no id: reject ends the frontmost call");
        c.take(&json!({ "type": "call", "phase": "ended" }), 0);
        assert_eq!(c.view(1_000, Language::De).state, "idle");
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
        let v = c.view(1_000, Language::De);
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

    #[test]
    fn elapsed_comes_with_a_change_not_every_second() {
        let mut c = Calls::default();
        c.take(&call(4, 2, "a"), 100);
        c.muted = true;
        c.take(&call(5, 2, "a"), 160);
        let v = c.view(170, Language::De);
        assert_eq!((v.since, v.elapsed), (100, 60));
        assert_eq!(
            c.view(999, Language::De).elapsed,
            60,
            "the page counts on itself"
        );
    }

    #[test]
    fn ending_stays_a_moment_then_idle() {
        let mut c = Calls::default();
        c.take(&call(4, 2, "a"), 100);
        c.take(
            &json!({ "type": "call", "phase": "ended", "status": 0, "uuid": "a" }),
            200,
        );
        assert_eq!(c.view(200, Language::De).state, "ending");
        assert!(c.ending(201));
        assert_eq!(c.view(200 + ENDING_SECS, Language::De).state, "idle");
        let mut fresh = Calls::default();
        fresh.take(
            &json!({ "type": "callLists", "recents": [], "favorites": [] }),
            5,
        );
        assert_eq!(
            fresh.view(5, Language::De).state,
            "idle",
            "no call, nothing ends"
        );
    }

    #[test]
    fn missed_calls_on_top_count_until_seen() {
        let mut c = Calls::default();
        c.take(&json!({ "type": "callLists", "recents": [
            { "number": "1", "name": "", "kind": "missed", "time": 30, "count": 2 },
            { "number": "2", "name": "", "kind": "missed", "time": 20, "count": 1 },
            { "number": "3", "name": "", "kind": "in", "time": 10, "count": 1 },
            { "number": "4", "name": "", "kind": "missed", "time": 5, "count": 1 } ], "favorites": [] }), 40);
        assert_eq!(c.view(40, Language::De).missed, 3);
        c.seen();
        assert_eq!(c.view(40, Language::De).missed, 0);
        c.take(&json!({ "type": "callLists", "recents": [
            { "number": "5", "name": "", "kind": "missed", "time": 50, "count": 1 },
            { "number": "1", "name": "", "kind": "missed", "time": 30, "count": 2 } ], "favorites": [] }), 60);
        assert_eq!(c.view(60, Language::De).missed, 1);
    }

    #[test]
    fn initials_as_the_board_service_makes_them() {
        assert_eq!(initials("Anna Beispiel"), "AB");
        assert_eq!(initials("Emil Erfunden, Junior"), "EJ");
        assert_eq!(initials("zoë"), "Z");
        assert_eq!(initials("+41 79 000 00 01"), "");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn times_read_as_on_the_phone() {
        let today = (2026, 11, 3, 20, 15);
        let yesterday = (2026, 11, 2, 20, 15);
        assert_eq!(
            when_text((2026, 11, 3, 9, 5), today, yesterday, Language::De),
            "09:05"
        );
        assert_eq!(
            when_text((2026, 11, 2, 23, 59), today, yesterday, Language::De),
            "Gestern"
        );
        assert_eq!(
            when_text((2026, 11, 2, 23, 59), today, yesterday, Language::En),
            "Yesterday"
        );
        assert_eq!(
            when_text((2026, 10, 28, 8, 0), today, yesterday, Language::De),
            "28.10."
        );
        assert_eq!(
            when_text((2026, 10, 28, 8, 0), today, yesterday, Language::En),
            "28/10"
        );
    }

    #[test]
    fn the_lists_carry_their_time_written_out_and_initials() {
        let mut c = Calls::default();
        let now = 1_790_000_000;
        c.take(&json!({ "type": "callLists",
            "recents": [ { "number": "1", "name": "Anna Beispiel", "kind": "in", "time": now - 60, "count": 1 } ],
            "favorites": [ { "number": "2", "name": "Bruno Muster", "label": "home" } ] }), now);
        let v = c.view(now, Language::De);
        assert_eq!(v.recents[0].initials, "AB");
        assert_eq!(
            v.recents[0].when.len(),
            5,
            "HH:MM today: {}",
            v.recents[0].when
        );
        assert_eq!(
            (
                v.favorites[0].initials.as_str(),
                v.favorites[0].when.as_str()
            ),
            ("BM", "")
        );
    }
}
