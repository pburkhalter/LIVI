//! LIVI as the notification server of the session it runs in: org.freedesktop.Notifications on the session bus
//! (Desktop Notifications Specification 1.2). Programs notify with the usual tools (notify-send, libnotify, busctl);
//! their notices land in the state for the UI, where the user closes them (Action::DismissNotice). Without a session
//! bus, or while another notification server holds the name, LIVI tries again now and then.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use livi_core_proto::state::Notice;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::server::Core;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The most notices kept; the oldest go first, critical ones last
const KEEP: usize = 50;
/// The longest summary and body kept, in characters
const SUMMARY_MAX: usize = 200;
const BODY_MAX: usize = 2000;
/// How long a transient notice without its own timeout stays
const TRANSIENT_MS: u64 = 8000;
/// Waits between attempts to get the name on the bus
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// Reasons of NotificationClosed
const EXPIRED: u32 = 1;
const DISMISSED: u32 = 2;
const CLOSED: u32 = 3;
const UNDEFINED: u32 = 4;

static BUS: OnceLock<zbus::Connection> = OnceLock::new();

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Ids go on from a start taken from the clock, so that an id a program kept from an earlier run of LIVI does not
/// name another program's notice
fn first_id() -> u32 {
    ((now_ms() / 1000) as u32 & 0x7fff_ffff).max(1)
}

/// What the server hands out: ids, the serials of changes, and the times of Notify calls
struct Counters {
    id: AtomicU32,
    serial: AtomicU32,
    time: AtomicU64,
}

impl Counters {
    fn new(first_id: u32) -> Self {
        Self {
            id: AtomicU32::new(first_id),
            serial: AtomicU32::new(1),
            time: AtomicU64::new(0),
        }
    }

    /// Unix milliseconds of a Notify, later than the one before even within one millisecond or after the clock went
    /// back: the expiry of a notice checks it to leave a newer version alone
    fn stamp(&self) -> u64 {
        let now = now_ms();
        let before = self
            .time
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |t| {
                Some(now.max(t + 1))
            })
            .unwrap_or(0);
        now.max(before + 1)
    }

    /// A new id no notice in the list has (never 0)
    fn new_id(&self, list: &[Notice]) -> u32 {
        loop {
            let id = self.id.fetch_add(1, Ordering::Relaxed);
            if id != 0 && list.iter().all(|n| n.id != id) {
                return id;
            }
        }
    }
}

/// Puts a notice into the list and returns its id and the ids of notices that had to go to keep the list short.
/// `replaces_id` names the notice it replaces; one that is gone (or never was here) comes anew under that id, as the
/// specification wants the same id back. Without it the notice gets a new id. A notice that says something new goes
/// to the end with a new serial; one that only repeats an old one keeps its place and serial.
fn put(
    list: &mut Vec<Notice>,
    mut notice: Notice,
    replaces_id: u32,
    counters: &Counters,
) -> (u32, Vec<u32>) {
    match list
        .iter()
        .position(|n| replaces_id != 0 && n.id == replaces_id)
    {
        Some(i) => {
            let kept = &mut list[i];
            notice.id = kept.id;
            notice.serial = kept.serial;
            let repeat = Notice {
                time: kept.time,
                ..notice.clone()
            } == *kept;
            if repeat {
                kept.time = notice.time;
                return (kept.id, Vec::new());
            }
            list.remove(i);
        }
        None if replaces_id != 0 => notice.id = replaces_id,
        None => notice.id = counters.new_id(list),
    }
    notice.serial = counters.serial.fetch_add(1, Ordering::Relaxed);
    let id = notice.id;
    list.push(notice);
    let mut gone = Vec::new();
    while list.len() > KEEP {
        let oldest = list.iter().position(|n| n.urgency < 2).unwrap_or(0);
        gone.push(list.remove(oldest).id);
    }
    (id, gone)
}

/// The first `max` characters
fn cut(text: String, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_string(),
        None => text,
    }
}

/// The urgency hint: a byte by the specification; some tools send another integer
fn urgency(hint: Option<&OwnedValue>) -> u8 {
    let Some(v) = hint else { return 1 };
    let n = v
        .downcast_ref::<u8>()
        .map(i64::from)
        .or_else(|_| v.downcast_ref::<i32>().map(i64::from))
        .or_else(|_| v.downcast_ref::<u32>().map(i64::from))
        .or_else(|_| v.downcast_ref::<i64>())
        .or_else(|_| v.downcast_ref::<u64>().map(|n| n.min(2) as i64))
        .or_else(|_| v.downcast_ref::<i16>().map(i64::from))
        .or_else(|_| v.downcast_ref::<u16>().map(i64::from))
        .unwrap_or(1);
    n.clamp(0, 2) as u8
}

/// Takes the notice `id` out of the state; false if it was not there
fn remove(core: &Core, id: u32, only_if_sent_at: Option<u64>) -> bool {
    let mut gone = false;
    core.hub.update(|s| {
        let before = s.notices.len();
        s.notices
            .retain(|n| n.id != id || only_if_sent_at.is_some_and(|t| n.time != t));
        gone = s.notices.len() != before;
    });
    gone
}

async fn closed(id: u32, reason: u32) {
    let Some(bus) = BUS.get() else { return };
    if let Ok(iface) = bus.object_server().interface::<_, Server>(PATH).await {
        let _ = Server::notification_closed(iface.signal_emitter(), id, reason).await;
    }
}

/// The user closed the notice in the UI
pub async fn dismiss(core: &Core, id: u32) {
    if remove(core, id, None) {
        closed(id, DISMISSED).await;
    }
}

struct Server {
    core: Arc<Core>,
    counters: Counters,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".into(), "persistence".into()]
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let urgency = urgency(hints.get("urgency"));
        let category = hints
            .get("category")
            .and_then(|v| v.downcast_ref::<&str>().ok().map(str::to_string))
            .unwrap_or_default();
        let transient = hints
            .get("transient")
            .and_then(|v| v.downcast_ref::<bool>().ok())
            .unwrap_or(false);
        let time = self.counters.stamp();
        let notice = Notice {
            id: 0,
            app: app_name,
            summary: cut(summary, SUMMARY_MAX),
            body: cut(body, BODY_MAX),
            urgency,
            category,
            time,
            serial: 0,
        };
        let (mut id, mut gone) = (0, Vec::new());
        self.core.hub.update(|s| {
            (id, gone) = put(&mut s.notices, notice, replaces_id, &self.counters);
        });
        for old in gone {
            closed(old, UNDEFINED).await;
        }
        // A timeout in ms lets it expire; -1 (the server decides) and 0 keep it until it is closed, as the list is the
        // place to look later, unless it is transient (it bypasses the persistence). A critical notice does not
        // expire.
        let after = match expire_timeout {
            ms if ms > 0 => Some(ms as u64),
            -1 if transient => Some(TRANSIENT_MS),
            _ => None,
        };
        if let Some(ms) = after.filter(|_| urgency < 2) {
            let core = self.core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                if remove(&core, id, Some(time)) {
                    closed(id, EXPIRED).await;
                }
            });
        }
        id
    }

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if !remove(&self.core, id, None) {
            // the specification answers an unknown id with an empty error
            return Err(zbus::fdo::Error::Failed(String::new()));
        }
        let _ = Self::notification_closed(&emitter, id, CLOSED).await;
        Ok(())
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "LIVI".into(),
            "LIVI".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.2".into(),
        )
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}

async fn connect(core: Arc<Core>) -> zbus::Result<zbus::Connection> {
    let server = Server {
        core,
        counters: Counters::new(first_id()),
    };
    zbus::connection::Builder::session()?
        .name(NAME)?
        .serve_at(PATH, server)?
        .build()
        .await
}

/// Serves the notifications as long as LIVI runs. The session bus may come up after LIVI, and another server may
/// hold the name for a while: it tries again, less often the longer it fails.
pub async fn serve(core: Arc<Core>) {
    let mut wait = RETRY_FIRST;
    let mut told = String::new();
    loop {
        match connect(core.clone()).await {
            Ok(bus) => {
                println!("[core] notifications: {NAME} on the session bus");
                let _ = BUS.set(bus);
                return;
            }
            Err(e) => {
                let why = e.to_string();
                if why != told {
                    println!("[core] notifications: not served yet ({why}), trying again");
                    told = why;
                }
            }
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(RETRY_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(app: &str, category: &str, summary: &str) -> Notice {
        Notice {
            app: app.into(),
            category: category.into(),
            summary: summary.into(),
            ..Notice::default()
        }
    }

    fn id(list: &mut Vec<Notice>, n: Notice, replaces_id: u32, counters: &Counters) -> u32 {
        put(list, n, replaces_id, counters).0
    }

    #[test]
    fn a_notice_replaces_only_the_one_it_names() {
        let mut list = Vec::new();
        let counters = Counters::new(10);
        let a = id(
            &mut list,
            notice("mail", "email.arrived", "A"),
            0,
            &counters,
        );
        let b = id(
            &mut list,
            notice("mail", "email.arrived", "B"),
            0,
            &counters,
        );
        // the same program and category without replaces_id: a notice of its own
        assert_eq!((a, b), (10, 11));
        assert_eq!(id(&mut list, notice("mail", "", "A2"), a, &counters), a);
        assert_eq!(list.len(), 2);
        assert_eq!(list.last().map(|n| n.summary.as_str()), Some("A2"));
    }

    #[test]
    fn an_unknown_replaces_id_comes_back_as_the_id() {
        let mut list = Vec::new();
        let counters = Counters::new(10);
        // e.g. notify-send -r 9001 for a volume display: one notice, always 9001
        assert_eq!(
            id(&mut list, notice("osd", "", "55%"), 9001, &counters),
            9001
        );
        assert_eq!(
            id(&mut list, notice("osd", "", "60%"), 9001, &counters),
            9001
        );
        assert_eq!(list.len(), 1);
        // new ids go around the ones in use
        let counters = Counters::new(9001);
        assert_eq!(id(&mut list, notice("x", "", "y"), 0, &counters), 9002);
    }

    #[test]
    fn something_new_comes_last_a_repeat_stays() {
        let mut list = Vec::new();
        let counters = Counters::new(1);
        let a = id(
            &mut list,
            notice("updates", "x-t100.updates", "3"),
            0,
            &counters,
        );
        id(&mut list, notice("radio", "", "on air"), 0, &counters);
        let serials = |list: &[Notice]| list.iter().map(|n| (n.id, n.serial)).collect::<Vec<_>>();
        assert_eq!(serials(&list), [(1, 1), (2, 2)]);
        // the same again, later: same place, same serial, the new time
        let again = Notice {
            time: 5,
            ..notice("updates", "x-t100.updates", "3")
        };
        assert_eq!(id(&mut list, again, a, &counters), a);
        assert_eq!(serials(&list), [(1, 1), (2, 2)]);
        assert_eq!(list[0].time, 5);
        // something new: last, with the next serial
        let worse = Notice {
            urgency: 2,
            ..notice("updates", "x-t100.updates", "3")
        };
        id(&mut list, worse, a, &counters);
        assert_eq!(serials(&list), [(2, 2), (1, 3)]);
    }

    #[test]
    fn the_list_keeps_the_newest_and_the_critical() {
        let mut list = Vec::new();
        let counters = Counters::new(1);
        let critical = Notice {
            urgency: 2,
            ..notice("updates", "", "security")
        };
        let kept = id(&mut list, critical, 0, &counters);
        let mut gone = Vec::new();
        for i in 0..KEEP + 5 {
            gone.extend(put(&mut list, notice("a", "", &i.to_string()), 0, &counters).1);
        }
        assert_eq!(list.len(), KEEP);
        assert_eq!(list[0].id, kept);
        assert_eq!(list[1].summary, "6");
        // the ones that went are told (NotificationClosed)
        assert_eq!(gone.len(), 6);
    }

    #[test]
    fn notify_times_always_grow() {
        let counters = Counters::new(1);
        let (a, b) = (counters.stamp(), counters.stamp());
        assert!(b > a);
    }

    #[test]
    fn long_texts_are_cut_by_characters() {
        assert_eq!(cut("äöü".into(), 2), "äö");
        assert_eq!(cut("ab".into(), 5), "ab");
    }

    #[test]
    fn urgency_takes_other_integers() {
        let v = |n: OwnedValue| urgency(Some(&n));
        assert_eq!(v(OwnedValue::from(2u8)), 2);
        assert_eq!(v(OwnedValue::from(2i32)), 2);
        assert_eq!(v(OwnedValue::from(0u32)), 0);
        assert_eq!(v(OwnedValue::from(9i64)), 2);
        assert_eq!(urgency(None), 1);
    }

    #[test]
    fn ids_never_start_at_zero() {
        assert!(first_id() > 0);
    }
}
