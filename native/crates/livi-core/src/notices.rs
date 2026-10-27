//! LIVI as the notification server of the session it runs in: org.freedesktop.Notifications on the session bus
//! (Desktop Notifications Specification 1.2). Programs notify with the usual tools (notify-send, libnotify, busctl);
//! their notices land in the state for the UI, where the user closes them (Action::DismissNotice). Without a session
//! bus, or with another notification server on it, LIVI goes without.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use livi_core_proto::state::Notice;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::server::Core;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The most notices kept; the oldest go first
const KEEP: usize = 50;
/// Reasons of NotificationClosed
const EXPIRED: u32 = 1;
const DISMISSED: u32 = 2;
const CLOSED: u32 = 3;

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

/// What the server hands out: ids, and the serials of changes
struct Counters {
    id: AtomicU32,
    serial: AtomicU32,
}

impl Counters {
    fn new(first_id: u32) -> Self {
        Self {
            id: AtomicU32::new(first_id),
            serial: AtomicU32::new(1),
        }
    }
}

/// Puts a notice into the list and returns its id. It replaces the notice `replaces_id` if that is still there, else
/// the same program's notice of the same category, and keeps its id; otherwise it gets a new one. A notice that says
/// something new goes to the end with a new serial; one that only repeats an old one keeps its place and serial.
fn put(list: &mut Vec<Notice>, mut notice: Notice, replaces_id: u32, counters: &Counters) -> u32 {
    let old = list
        .iter()
        .position(|n| replaces_id != 0 && n.id == replaces_id)
        .or_else(|| {
            list.iter().position(|n| {
                !notice.category.is_empty() && n.app == notice.app && n.category == notice.category
            })
        });
    if let Some(i) = old {
        let kept = &mut list[i];
        notice.id = kept.id;
        notice.serial = kept.serial;
        let repeat = Notice {
            time: kept.time,
            ..notice.clone()
        } == *kept;
        if repeat {
            kept.time = notice.time;
            return kept.id;
        }
        list.remove(i);
    } else {
        notice.id = counters.id.fetch_add(1, Ordering::Relaxed);
    }
    notice.serial = counters.serial.fetch_add(1, Ordering::Relaxed);
    let id = notice.id;
    list.push(notice);
    if list.len() > KEEP {
        list.drain(..list.len() - KEEP);
    }
    id
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
        let urgency = hints
            .get("urgency")
            .and_then(|v| v.downcast_ref::<u8>().ok())
            .unwrap_or(1)
            .min(2);
        let category = hints
            .get("category")
            .and_then(|v| v.downcast_ref::<&str>().ok().map(str::to_string))
            .unwrap_or_default();
        let time = now_ms();
        let notice = Notice {
            id: 0,
            app: app_name,
            summary,
            body,
            urgency,
            category,
            time,
            serial: 0,
        };
        let mut id = 0;
        self.core.hub.update(|s| {
            id = put(&mut s.notices, notice, replaces_id, &self.counters);
        });
        // A timeout in ms lets it expire; -1 (the server decides) and 0 keep it until it is closed, as the list is the
        // place to look later. A critical notice does not expire.
        if expire_timeout > 0 && urgency < 2 {
            let core = self.core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(expire_timeout as u64)).await;
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
    ) {
        if remove(&self.core, id, None) {
            let _ = Self::notification_closed(&emitter, id, CLOSED).await;
        }
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

/// Serves the notifications as long as LIVI runs
pub async fn serve(core: Arc<Core>) {
    match connect(core).await {
        Ok(bus) => {
            println!("[core] notifications: {NAME} on the session bus");
            let _ = BUS.set(bus);
        }
        Err(e) => println!("[core] notifications: not served ({e})"),
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

    #[test]
    fn a_notice_replaces_by_id_or_by_program_and_category() {
        let mut list = Vec::new();
        let counters = Counters::new(10);
        let a = put(
            &mut list,
            notice("updates", "x-t100.updates", "3"),
            0,
            &counters,
        );
        let b = put(&mut list, notice("radio", "", "on air"), 0, &counters);
        assert_eq!((a, b), (10, 11));
        let changed = notice("updates", "x-t100.updates", "4");
        assert_eq!(put(&mut list, changed, 0, &counters), a);
        assert_eq!(
            put(&mut list, notice("radio", "", "off air"), b, &counters),
            b
        );
        assert_eq!(list.len(), 2);
        assert_eq!(list.last().map(|n| n.summary.as_str()), Some("off air"));
        // a category of another program, or an id that is gone, makes a new notice
        let other = notice("other", "x-t100.updates", "x");
        assert_eq!(put(&mut list, other, 99, &counters), 12);
    }

    #[test]
    fn something_new_comes_last_a_repeat_stays() {
        let mut list = Vec::new();
        let counters = Counters::new(1);
        let a = put(
            &mut list,
            notice("updates", "x-t100.updates", "3"),
            0,
            &counters,
        );
        put(&mut list, notice("radio", "", "on air"), 0, &counters);
        let serials = |list: &[Notice]| list.iter().map(|n| (n.id, n.serial)).collect::<Vec<_>>();
        assert_eq!(serials(&list), [(1, 1), (2, 2)]);
        // the same again, later: same place, same serial, the new time
        let again = Notice {
            time: 5,
            ..notice("updates", "x-t100.updates", "3")
        };
        assert_eq!(put(&mut list, again, a, &counters), a);
        assert_eq!(serials(&list), [(1, 1), (2, 2)]);
        assert_eq!(list[0].time, 5);
        // something new: last, with the next serial
        let worse = Notice {
            urgency: 2,
            ..notice("updates", "x-t100.updates", "3")
        };
        put(&mut list, worse, a, &counters);
        assert_eq!(serials(&list), [(2, 2), (1, 3)]);
    }

    #[test]
    fn the_list_keeps_the_newest() {
        let mut list = Vec::new();
        let counters = Counters::new(1);
        for i in 0..KEEP + 5 {
            put(&mut list, notice("a", "", &i.to_string()), 0, &counters);
        }
        assert_eq!(list.len(), KEEP);
        assert_eq!(list[0].summary, "5");
    }

    #[test]
    fn ids_never_start_at_zero() {
        assert!(first_id() > 0);
    }
}
