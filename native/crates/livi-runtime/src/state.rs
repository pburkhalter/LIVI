use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::livi_sock::SharedTag;
use crate::vehicle::{Vehicle, VehicleFeed};
use tokio::sync::Notify;

#[derive(Default)]
pub struct HelperState {
    reconnect_targets: Mutex<Vec<(String, Option<String>)>>,
    carkit: Mutex<Vec<SharedTag>>,
    links: Mutex<HashSet<String>>,
    vehicle: Vehicle,
    wired: Mutex<HashMap<String, Wired>>,
    redo: Mutex<HashSet<String>>,
    bt: Mutex<HashMap<String, BtEntry>>,
    bt_next: AtomicU64,
}

/// A Bluetooth iAP2 link of the helper's. `close`: the link's socket shuts down (driver `spawn_link_closable`). `id`
/// tells it from a later link to the same phone
#[derive(Clone, Default)]
pub struct BtLink {
    pub id: u64,
    pub close: Arc<Notify>,
}

struct BtEntry {
    link: BtLink,
}

struct Wired {
    restart: Arc<Notify>,
    again: Arc<Notify>,
    reset: Arc<Notify>,
}

impl HelperState {
    pub fn vehicle(&self) -> &Vehicle {
        &self.vehicle
    }

    pub fn vehicle_feed(&self) -> VehicleFeed {
        self.vehicle.feed()
    }

    pub fn wired_started(
        &self,
        serial: &str,
        restart: Arc<Notify>,
        again: Arc<Notify>,
    ) -> Arc<Notify> {
        let reset = Arc::new(Notify::new());
        self.wired.lock().unwrap().insert(
            serial.to_string(),
            Wired {
                restart,
                again,
                reset: reset.clone(),
            },
        );
        reset
    }

    pub fn reset_wired(&self, serial: &str) -> bool {
        let wired = self.wired.lock().unwrap();
        wired
            .get(serial)
            .inspect(|w| w.reset.notify_one())
            .is_some()
    }

    pub fn wired_ended(&self, serial: &str) {
        self.wired.lock().unwrap().remove(serial);
    }

    pub fn restart_wired(&self) -> usize {
        let wired = self.wired.lock().unwrap();
        let mut redo = self.redo.lock().unwrap();
        for (serial, w) in wired.iter() {
            redo.insert(serial.clone());
            w.restart.notify_one();
        }
        wired.len()
    }

    pub fn start_wired_again(&self, serial: &str) -> bool {
        let wired = self.wired.lock().unwrap();
        wired
            .get(serial)
            .inspect(|w| w.again.notify_one())
            .is_some()
    }

    pub fn take_redo(&self, serial: &str) -> bool {
        self.redo.lock().unwrap().remove(serial)
    }

    pub fn link_up(&self, mac: &str) {
        self.links.lock().unwrap().insert(mac.to_uppercase());
    }

    pub fn link_down(&self, mac: &str) {
        self.links.lock().unwrap().remove(&mac.to_uppercase());
    }

    pub fn link_active(&self, mac: &str) -> bool {
        self.links.lock().unwrap().contains(&mac.to_uppercase())
    }

    /// A Bluetooth iAP2 link to `mac` began; it stays known until `bt_link_ended` with it
    pub fn bt_link_started(&self, mac: &str) -> BtLink {
        let link = BtLink {
            id: self.bt_next.fetch_add(1, Ordering::Relaxed) + 1,
            ..Default::default()
        };
        self.bt.lock().unwrap().insert(
            mac.to_uppercase(),
            BtEntry { link: link.clone() },
        );
        link
    }

    /// The link ended; a later link to the same phone stays
    pub fn bt_link_ended(&self, mac: &str, link: &BtLink) {
        let mut bt = self.bt.lock().unwrap();
        let key = mac.to_uppercase();
        if bt.get(&key).is_some_and(|e| e.link.id == link.id) {
            bt.remove(&key);
        }
    }

    pub fn bt_link_active(&self, mac: &str) -> bool {
        self.bt.lock().unwrap().contains_key(&mac.to_uppercase())
    }

    /// The Bluetooth iAP2 link to `mac` shuts down, only the one with `link` when named; false: there is none
    pub fn close_bt(&self, mac: &str, link: Option<u64>) -> bool {
        let bt = self.bt.lock().unwrap();
        bt.get(&mac.to_uppercase())
            .filter(|e| link.is_none_or(|id| id == e.link.id))
            .inspect(|e| e.link.close.notify_one())
            .is_some()
    }

    pub fn set_reconnect_targets(&self, targets: Vec<(String, Option<String>)>) {
        let normalized = targets
            .into_iter()
            .map(|(m, u)| (m.to_uppercase(), u))
            .collect();
        *self.reconnect_targets.lock().unwrap() = normalized;
    }

    pub fn reconnect_targets(&self) -> Vec<(String, Option<String>)> {
        self.reconnect_targets.lock().unwrap().clone()
    }

    pub fn carkit_started(&self, tag: SharedTag) {
        self.carkit.lock().unwrap().push(tag);
    }

    pub fn carkit_ended(&self, tag: &SharedTag) {
        self.carkit.lock().unwrap().retain(|t| !Arc::ptr_eq(t, tag));
    }

    pub fn carkit_blocks(&self, bt_mac: &str) -> bool {
        let sessions = self.carkit.lock().unwrap();
        if bt_mac.is_empty() {
            return !sessions.is_empty();
        }
        sessions.iter().any(|t| {
            t.lock()
                .unwrap()
                .phone_id
                .as_deref()
                .is_some_and(|p| p.eq_ignore_ascii_case(bt_mac))
        })
    }

    pub fn carkit_claims(&self, bt_mac: &str) -> bool {
        let sessions = self.carkit.lock().unwrap();
        sessions.iter().any(|t| {
            t.lock()
                .unwrap()
                .phone_id
                .as_deref()
                .is_none_or(|p| p.eq_ignore_ascii_case(bt_mac))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventTag;

    fn tag(mac: Option<&str>) -> SharedTag {
        Arc::new(Mutex::new(EventTag {
            phone_id: mac.map(str::to_string),
            ..Default::default()
        }))
    }

    #[test]
    fn blocks_only_the_wired_phone() {
        let state = HelperState::default();
        let wired = tag(Some("0C:6A:C4:4E:F3:2A"));
        state.carkit_started(wired.clone());
        assert!(state.carkit_blocks("0c:6a:c4:4e:f3:2a"));
        assert!(!state.carkit_blocks("AA:BB:CC:DD:EE:FF"));
        state.carkit_ended(&wired);
        assert!(!state.carkit_blocks("0C:6A:C4:4E:F3:2A"));
    }

    #[test]
    fn no_mac_falls_back_to_any_carkit_session() {
        let state = HelperState::default();
        assert!(!state.carkit_blocks(""));
        let unlearned = tag(None);
        state.carkit_started(unlearned.clone());
        assert!(state.carkit_blocks(""));
        assert!(!state.carkit_blocks("AA:BB:CC:DD:EE:FF"));
        state.carkit_ended(&unlearned);
        assert!(!state.carkit_blocks(""));
    }

    #[test]
    fn carkit_claims_its_phone_and_any_it_has_not_identified() {
        let state = HelperState::default();
        assert!(!state.carkit_claims("0c:6a:c4:4e:f3:2a"));
        let unidentified = tag(None);
        state.carkit_started(unidentified.clone());
        assert!(state.carkit_claims("AA:BB:CC:DD:EE:FF"));
        unidentified.lock().unwrap().phone_id = Some("0C:6A:C4:4E:F3:2A".into());
        assert!(state.carkit_claims("0c:6a:c4:4e:f3:2a"));
        assert!(!state.carkit_claims("AA:BB:CC:DD:EE:FF"));
        state.carkit_ended(&unidentified);
        assert!(!state.carkit_claims("0c:6a:c4:4e:f3:2a"));
    }

    #[tokio::test]
    async fn a_wired_session_is_asked_for_another_start_without_being_ended() {
        let state = HelperState::default();
        let (restart, again) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        state.wired_started("00008120", restart.clone(), again.clone());

        assert!(!state.start_wired_again("00008030"));
        assert!(state.start_wired_again("00008120"));
        again.notified().await;
        assert!(!state.take_redo("00008120"));

        state.wired_ended("00008120");
        assert!(!state.start_wired_again("00008120"));
    }

    #[tokio::test]
    async fn a_stuck_wired_session_is_asked_for_a_usb_reset() {
        let state = HelperState::default();
        let reset =
            state.wired_started("00008120", Arc::new(Notify::new()), Arc::new(Notify::new()));

        assert!(!state.reset_wired("00008030"));
        assert!(state.reset_wired("00008120"));
        reset.notified().await;

        state.wired_ended("00008120");
        assert!(!state.reset_wired("00008120"));
    }

    #[tokio::test]
    async fn only_the_link_named_is_shut_and_its_end_leaves_a_later_one() {
        let state = HelperState::default();
        let old = state.bt_link_started("AA:BB:CC:DD:EE:FF");
        let new = state.bt_link_started("aa:bb:cc:dd:ee:ff");
        state.bt_link_ended("AA:BB:CC:DD:EE:FF", &old);
        assert!(state.bt_link_active("AA:BB:CC:DD:EE:FF"));
        assert!(!state.close_bt("aa:bb:cc:dd:ee:ff", Some(old.id)));
        assert!(state.close_bt("aa:bb:cc:dd:ee:ff", Some(new.id)));
        new.close.notified().await;
        assert!(state.close_bt("aa:bb:cc:dd:ee:ff", None));
        state.bt_link_ended("AA:BB:CC:DD:EE:FF", &new);
        assert!(!state.close_bt("AA:BB:CC:DD:EE:FF", None));
    }

    #[test]
    fn a_phone_with_a_link_is_known_whatever_the_case_of_its_mac() {
        let state = HelperState::default();
        assert!(!state.link_active("0c:6a:c4:4e:f3:2a"));
        state.link_up("0c:6a:c4:4e:f3:2a");
        assert!(state.link_active("0C:6A:C4:4E:F3:2A"));
        state.link_down("0C:6A:C4:4E:F3:2A");
        assert!(!state.link_active("0c:6a:c4:4e:f3:2a"));
    }
}
