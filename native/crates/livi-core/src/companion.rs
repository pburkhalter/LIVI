//! The companion channel between UI plugins and programs beside LIVI (a voice assistant, a radio, a CAN reader):
//! a client of the core socket publishes a small state of its own under a topic (Action::SetExtension, state
//! `extensions`), which plugins read; plugins send it events (Action::Emit), which the core hands to the clients that
//! subscribed the topic in their hello (FromCore::Event). Nothing of it is saved: a topic goes with the client that
//! published it, and an event reaches only who listens at that moment.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;
use tokio::sync::broadcast;

use crate::hub::Hub;

/// The most a topic's state may take as JSON, in bytes
pub const STATE_MAX: usize = 4096;
/// The most a value of an event may take as JSON, in bytes
pub const EVENT_MAX: usize = 4096;
/// The most topics one client publishes
pub const TOPICS_MAX: usize = 8;
const NAME_MAX: usize = 64;
/// Events a slow client may lag behind before it misses some
const BACKLOG: usize = 32;

/// A connection to the core, as the companion channel tells clients apart
pub type ClientId = u64;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub topic: String,
    pub name: String,
    pub value: Value,
}

pub struct Companion {
    /// Who publishes each topic
    owners: Mutex<HashMap<String, ClientId>>,
    events: broadcast::Sender<Event>,
}

/// A topic or an event's name: letters, digits, `-` and `_` (a plugin reads `<topic>.<key>`)
fn check_name(what: &str, name: &str) -> Result<(), String> {
    let fine = !name.is_empty()
        && name.len() <= NAME_MAX
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if fine {
        Ok(())
    } else {
        Err(format!(
            "{what} {name:?}: 1 to {NAME_MAX} letters, digits, - or _"
        ))
    }
}

fn size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |v| v.len())
}

impl Default for Companion {
    fn default() -> Self {
        Self {
            owners: Mutex::new(HashMap::new()),
            events: broadcast::channel(BACKLOG).0,
        }
    }
}

impl Companion {
    fn owners(&self) -> std::sync::MutexGuard<'_, HashMap<String, ClientId>> {
        self.owners.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The client's state under `topic` replaces the one before; null takes it away. A topic belongs to the client
    /// that published it first, until that client goes or takes it away.
    pub fn publish(
        &self,
        hub: &Hub,
        client: ClientId,
        topic: &str,
        value: Value,
    ) -> Result<(), String> {
        check_name("topic", topic)?;
        let mut owners = self.owners();
        if owners.get(topic).is_some_and(|&owner| owner != client) {
            return Err(format!("topic {topic} belongs to another client"));
        }
        match value {
            Value::Null => {
                if owners.remove(topic).is_some() {
                    hub.update(|s| {
                        s.extensions.remove(topic);
                    });
                }
                Ok(())
            }
            Value::Object(_) => {
                let n = size(&value);
                if n > STATE_MAX {
                    return Err(format!(
                        "state of {topic} takes {n} bytes, at most {STATE_MAX}"
                    ));
                }
                let mine = owners.values().filter(|&&owner| owner == client).count();
                if !owners.contains_key(topic) && mine >= TOPICS_MAX {
                    return Err(format!("at most {TOPICS_MAX} topics per client"));
                }
                owners.insert(topic.to_string(), client);
                hub.update(|s| {
                    s.extensions.insert(topic.to_string(), value);
                });
                Ok(())
            }
            _ => Err(format!("state of {topic} has to be an object or null")),
        }
    }

    /// The client went: its topics go from the state
    pub fn leave(&self, hub: &Hub, client: ClientId) {
        let mut owners = self.owners();
        let gone: Vec<String> = owners
            .iter()
            .filter(|&(_, &owner)| owner == client)
            .map(|(topic, _)| topic.clone())
            .collect();
        if gone.is_empty() {
            return;
        }
        for topic in &gone {
            owners.remove(topic);
        }
        hub.update(|s| {
            for topic in &gone {
                s.extensions.remove(topic);
            }
        });
    }

    /// Hands an event to whoever listens to the topic now; nobody listening is no error
    pub fn emit(&self, topic: &str, name: &str, value: Value) -> Result<(), String> {
        check_name("topic", topic)?;
        check_name("event", name)?;
        let n = size(&value);
        if n > EVENT_MAX {
            return Err(format!(
                "value of {topic} {name} takes {n} bytes, at most {EVENT_MAX}"
            ));
        }
        let _ = self.events.send(Event {
            topic: topic.into(),
            name: name.into(),
            value,
        });
        Ok(())
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use livi_core_proto::message::FromCore;
    use livi_core_proto::patch::PatchOp;
    use serde_json::json;

    use super::*;
    use crate::hub::tests::{decode, hub};

    fn extensions(hub: &Hub) -> serde_json::Map<String, Value> {
        hub.watch().borrow().extensions.clone()
    }

    #[test]
    fn a_topic_is_the_publishers_until_it_goes() {
        let (hub, c) = (hub(), Companion::default());
        c.publish(&hub, 1, "radio", json!({ "station": "SRF 3" }))
            .unwrap();
        c.publish(&hub, 1, "radio", json!({ "station": "DRS" }))
            .unwrap();
        assert_eq!(extensions(&hub)["radio"], json!({ "station": "DRS" }));
        assert!(
            c.publish(&hub, 2, "radio", json!({ "station": "x" }))
                .is_err()
        );
        assert!(c.publish(&hub, 2, "radio", Value::Null).is_err());
        c.publish(&hub, 2, "doors", json!({ "open": true }))
            .unwrap();
        c.leave(&hub, 1);
        assert!(!extensions(&hub).contains_key("radio"));
        assert!(extensions(&hub).contains_key("doors"));
        // free again
        c.publish(&hub, 2, "radio", json!({})).unwrap();
        c.publish(&hub, 2, "radio", Value::Null).unwrap();
        assert!(!extensions(&hub).contains_key("radio"));
        c.publish(&hub, 3, "radio", json!({})).unwrap();
    }

    #[test]
    fn a_state_is_a_small_object_under_a_plain_name() {
        let (hub, c) = (hub(), Companion::default());
        for bad in ["", "a.b", "a b", "ä", &"x".repeat(65)] {
            assert!(c.publish(&hub, 1, bad, json!({})).is_err(), "{bad:?}");
        }
        assert!(c.publish(&hub, 1, "radio", json!("on")).is_err());
        assert!(c.publish(&hub, 1, "radio", json!([1])).is_err());
        let big = json!({ "text": "x".repeat(STATE_MAX) });
        assert!(c.publish(&hub, 1, "radio", big).is_err());
        assert!(extensions(&hub).is_empty());
        for i in 0..TOPICS_MAX {
            c.publish(&hub, 1, &format!("t{i}"), json!({})).unwrap();
        }
        assert!(c.publish(&hub, 1, "one-more", json!({})).is_err());
        c.publish(&hub, 1, "t0", json!({ "again": 1 })).unwrap();
    }

    #[test]
    fn a_change_patches_only_what_changed() {
        let (hub, c) = (hub(), Companion::default());
        c.publish(
            &hub,
            1,
            "voice",
            json!({ "state": "listening", "level": 0.2 }),
        )
        .unwrap();
        let (_, mut patches) = hub.welcome();
        c.publish(
            &hub,
            1,
            "voice",
            json!({ "state": "listening", "level": 0.7 }),
        )
        .unwrap();
        let FromCore::Patch { ops, .. } = decode(&patches.try_recv().unwrap()) else {
            panic!("not a patch")
        };
        let level = PatchOp::Set {
            path: vec!["extensions".into(), "voice".into(), "level".into()],
            value: json!(0.7),
        };
        assert_eq!(ops, [level]);
        // the same again changes nothing
        c.publish(
            &hub,
            1,
            "voice",
            json!({ "state": "listening", "level": 0.7 }),
        )
        .unwrap();
        assert!(patches.try_recv().is_err());
    }

    #[test]
    fn an_event_reaches_who_listens() {
        let c = Companion::default();
        c.emit("voice", "press", json!("")).unwrap();
        let mut rx = c.events();
        c.emit("voice", "press", json!("")).unwrap();
        assert_eq!(
            rx.try_recv().unwrap(),
            Event {
                topic: "voice".into(),
                name: "press".into(),
                value: json!(""),
            }
        );
        assert!(c.emit("voice", "", json!("")).is_err());
        assert!(c.emit("a.b", "press", json!("")).is_err());
        assert!(
            c.emit("voice", "say", json!("x".repeat(EVENT_MAX)))
                .is_err()
        );
        assert!(rx.try_recv().is_err());
    }
}
