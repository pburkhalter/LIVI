//! Whether Wi-Fi and Bluetooth are off, on or connected to a phone: `links` in the state, e.g. for status icons.
//!
//! Bluetooth follows BlueZ on the system bus (org.bluez and its object manager): off while no adapter is powered,
//! connected while a phone is connected, otherwise on. Without the system bus or BlueZ it reads off; the watcher tries
//! again now and then, and follows BlueZ's name on the bus when bluetoothd starts or stops later. Wi-Fi comes from what
//! the core knows already (`wifi`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use livi_core_proto::state::LinkState;
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{MatchRule, Message, MessageStream};

use crate::devices::phone_like;
use crate::hub::Hub;

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const DBUS: &str = "org.freedesktop.DBus";
/// Waits between attempts to reach the system bus
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// Properties of an interface by name
type Props = HashMap<String, OwnedValue>;
/// What GetManagedObjects returns: interfaces and their properties by object path
type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

/// Wi-Fi from what the core knows: off without a Wi-Fi interface (system.wifiInterfaces, which also lists the dongle's
/// radio while it answers), connected while a phone is present over Wi-Fi (a wireless session, or a known phone on
/// LIVI's access point), otherwise on. Whether the access point runs is not asked: the interface is what a phone can
/// come over.
pub fn wifi(interfaces: &[String], phone: bool) -> LinkState {
    match (interfaces.is_empty(), phone) {
        (true, _) => LinkState::Off,
        (false, true) => LinkState::Connected,
        (false, false) => LinkState::On,
    }
}

/// What the watcher keeps of BlueZ's objects, by object path
#[derive(Debug, Default)]
struct Radios {
    /// Adapter1: whether it is powered
    adapters: HashMap<String, bool>,
    /// Device1: whether it is connected, and its class of device (0 when it has none, e.g. Bluetooth LE)
    devices: HashMap<String, (bool, u32)>,
}

impl Radios {
    fn of(objects: Objects) -> Self {
        let mut radios = Self::default();
        for (path, interfaces) in objects {
            for (interface, props) in interfaces {
                radios.change(path.as_str(), &interface, &props);
            }
        }
        radios
    }

    /// An interface came with its properties (InterfacesAdded), or some of them changed (PropertiesChanged)
    fn change(&mut self, path: &str, interface: &str, props: &Props) {
        let flag = |key: &str| props.get(key).and_then(|v| v.downcast_ref::<bool>().ok());
        match interface {
            ADAPTER => {
                let powered = self.adapters.entry(path.into()).or_default();
                if let Some(v) = flag("Powered") {
                    *powered = v;
                }
            }
            DEVICE => {
                let (connected, class) = self.devices.entry(path.into()).or_default();
                if let Some(v) = flag("Connected") {
                    *connected = v;
                }
                if let Some(v) = props
                    .get("Class")
                    .and_then(|v| v.downcast_ref::<u32>().ok())
                {
                    *class = v;
                }
            }
            _ => {}
        }
    }

    /// An object lost interfaces (InterfacesRemoved)
    fn remove(&mut self, path: &str, interfaces: &[String]) {
        for interface in interfaces {
            match interface.as_str() {
                ADAPTER => {
                    self.adapters.remove(path);
                }
                DEVICE => {
                    self.devices.remove(path);
                }
                _ => {}
            }
        }
    }

    /// A phone counts as LIVI counts phones elsewhere: not an audio device (devices::phone_like)
    fn state(&self) -> LinkState {
        let phone = |&(connected, class): &(bool, u32)| connected && phone_like(u64::from(class));
        if !self.adapters.values().any(|&powered| powered) {
            LinkState::Off
        } else if self.devices.values().any(phone) {
            LinkState::Connected
        } else {
            LinkState::On
        }
    }

    /// Takes in a signal of BlueZ; other signals are left alone
    fn signal(&mut self, msg: &Message) -> zbus::Result<()> {
        let header = msg.header();
        let (Some(interface), Some(member)) = (header.interface(), header.member()) else {
            return Ok(());
        };
        match (interface.as_str(), member.as_str()) {
            (OBJECT_MANAGER, "InterfacesAdded") => {
                let (path, interfaces): (OwnedObjectPath, HashMap<String, Props>) =
                    msg.body().deserialize()?;
                for (interface, props) in interfaces {
                    self.change(path.as_str(), &interface, &props);
                }
            }
            (OBJECT_MANAGER, "InterfacesRemoved") => {
                let (path, interfaces): (OwnedObjectPath, Vec<String>) =
                    msg.body().deserialize()?;
                self.remove(path.as_str(), &interfaces);
            }
            (PROPERTIES, "PropertiesChanged") => {
                let Some(path) = header.path() else {
                    return Ok(());
                };
                let (interface, changed, _invalidated): (String, Props, Vec<String>) =
                    msg.body().deserialize()?;
                self.change(path.as_str(), &interface, &changed);
            }
            _ => {}
        }
        Ok(())
    }
}

/// BlueZ's objects now; none while BlueZ is not on the bus
async fn read(bus: &zbus::Connection) -> Radios {
    let reply = bus
        .call_method(
            Some(BLUEZ),
            "/",
            Some(OBJECT_MANAGER),
            "GetManagedObjects",
            &(),
        )
        .await;
    reply
        .ok()
        .and_then(|r| r.body().deserialize::<Objects>().ok())
        .map(Radios::of)
        .unwrap_or_default()
}

fn publish(hub: &Hub, state: LinkState) {
    hub.update(|s| s.links.bluetooth = state);
}

/// Follows BlueZ until the connection to the system bus ends
async fn watch(hub: &Hub, wait: &mut Duration) -> zbus::Result<()> {
    let bus = zbus::Connection::system().await?;
    // the bus daemon matches the sender's well-known name: signals of whoever owns org.bluez
    let bluez = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(BLUEZ)?
        .build();
    let owner = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(DBUS)?
        .interface(DBUS)?
        .member("NameOwnerChanged")?
        .add_arg(BLUEZ)?
        .build();
    // subscribed before reading, so that no change in between goes missing
    let mut signals = MessageStream::for_match_rule(bluez, &bus, None).await?;
    let mut owners = MessageStream::for_match_rule(owner, &bus, None).await?;
    *wait = RETRY_FIRST;
    println!("[core] bluetooth: following BlueZ on the system bus");
    let mut radios = read(&bus).await;
    let mut shown = radios.state();
    publish(hub, shown);
    loop {
        tokio::select! {
            msg = owners.next() => {
                let Some(msg) = msg else { return Ok(()) };
                // bluetoothd started (again) or stopped: its objects are new or gone
                let (_, _, new): (String, String, String) = msg?.body().deserialize()?;
                radios = if new.is_empty() { Radios::default() } else { read(&bus).await };
            }
            msg = signals.next() => {
                let Some(msg) = msg else { return Ok(()) };
                // a signal that does not parse changes nothing
                if let Err(e) = msg.and_then(|m| radios.signal(&m)) {
                    eprintln!("[core] bluetooth: a signal of BlueZ unread: {e}");
                }
            }
        }
        let now = radios.state();
        if now != shown {
            shown = now;
            publish(hub, now);
        }
    }
}

/// Follows Bluetooth's state as long as LIVI runs. The system bus may come up after LIVI: it tries again, less often the
/// longer it fails; meanwhile Bluetooth reads off.
pub async fn follow(hub: Arc<Hub>) {
    let mut wait = RETRY_FIRST;
    let mut told = String::new();
    loop {
        let why = match watch(&hub, &mut wait).await {
            Ok(()) => "the system bus closed".to_string(),
            Err(e) => e.to_string(),
        };
        publish(&hub, LinkState::Off);
        if why != told {
            println!("[core] bluetooth: no BlueZ ({why}), trying again");
            told = why;
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(RETRY_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPHONE: u32 = 0x7a020c;
    const SPEAKER: u32 = 0x240414;

    fn props<const N: usize>(pairs: [(&str, OwnedValue); N]) -> Props {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn adapter(radios: &mut Radios, powered: bool) {
        radios.change(
            "/org/bluez/hci0",
            ADAPTER,
            &props([("Powered", powered.into())]),
        );
    }

    fn device(radios: &mut Radios, path: &str, connected: bool, class: u32) {
        radios.change(
            path,
            DEVICE,
            &props([("Connected", connected.into()), ("Class", class.into())]),
        );
    }

    #[test]
    fn wifi_needs_an_interface_and_a_phone_on_it() {
        let wlan = ["wlan0".to_string()];
        assert_eq!(wifi(&[], false), LinkState::Off);
        assert_eq!(wifi(&[], true), LinkState::Off);
        assert_eq!(wifi(&wlan, false), LinkState::On);
        assert_eq!(wifi(&wlan, true), LinkState::Connected);
    }

    #[test]
    fn bluetooth_is_off_without_a_powered_adapter() {
        let mut radios = Radios::default();
        assert_eq!(radios.state(), LinkState::Off);
        adapter(&mut radios, false);
        assert_eq!(radios.state(), LinkState::Off);
        adapter(&mut radios, true);
        assert_eq!(radios.state(), LinkState::On);
        radios.remove("/org/bluez/hci0", &[ADAPTER.into()]);
        assert_eq!(radios.state(), LinkState::Off);
    }

    #[test]
    fn bluetooth_is_connected_with_a_phone_not_with_a_speaker() {
        let mut radios = Radios::default();
        adapter(&mut radios, true);
        let phone = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
        device(
            &mut radios,
            "/org/bluez/hci0/dev_11_22_33_44_55_66",
            true,
            SPEAKER,
        );
        assert_eq!(radios.state(), LinkState::On);
        device(&mut radios, phone, false, IPHONE);
        assert_eq!(radios.state(), LinkState::On);
        // a change names only what changed: the class stays
        radios.change(phone, DEVICE, &props([("Connected", true.into())]));
        assert_eq!(radios.state(), LinkState::Connected);
        // switched off, the adapter rules (BlueZ then also disconnects its devices)
        adapter(&mut radios, false);
        assert_eq!(radios.state(), LinkState::Off);
        adapter(&mut radios, true);
        radios.remove(phone, &[DEVICE.into(), PROPERTIES.into()]);
        assert_eq!(radios.state(), LinkState::On);
    }

    #[test]
    fn a_device_without_a_class_counts_as_a_phone() {
        let mut radios = Radios::default();
        adapter(&mut radios, true);
        radios.change(
            "/org/bluez/hci0/dev_01",
            DEVICE,
            &props([("Connected", true.into())]),
        );
        assert_eq!(radios.state(), LinkState::Connected);
    }

    #[test]
    fn the_objects_of_bluez_are_read_at_once() {
        let mut objects = Objects::new();
        let path = |p: &str| OwnedObjectPath::try_from(p).unwrap();
        objects.insert(
            path("/org/bluez/hci0"),
            HashMap::from([
                (ADAPTER.to_string(), props([("Powered", true.into())])),
                ("org.bluez.Media1".to_string(), HashMap::new()),
            ]),
        );
        objects.insert(
            path("/org/bluez/hci0/dev_01"),
            HashMap::from([(
                DEVICE.to_string(),
                props([("Connected", true.into()), ("Class", IPHONE.into())]),
            )]),
        );
        assert_eq!(Radios::of(objects).state(), LinkState::Connected);
    }

    #[test]
    fn signals_of_bluez_change_the_state() {
        let mut radios = Radios::default();
        let added = Message::signal("/", OBJECT_MANAGER, "InterfacesAdded")
            .unwrap()
            .build(&(
                OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(),
                HashMap::from([(ADAPTER, props([("Powered", false.into())]))]),
            ))
            .unwrap();
        radios.signal(&added).unwrap();
        assert_eq!(radios.state(), LinkState::Off);
        let powered = Message::signal("/org/bluez/hci0", PROPERTIES, "PropertiesChanged")
            .unwrap()
            .build(&(
                ADAPTER,
                props([("Powered", true.into())]),
                Vec::<String>::new(),
            ))
            .unwrap();
        radios.signal(&powered).unwrap();
        assert_eq!(radios.state(), LinkState::On);
        let removed = Message::signal("/", OBJECT_MANAGER, "InterfacesRemoved")
            .unwrap()
            .build(&(
                OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(),
                vec![ADAPTER],
            ))
            .unwrap();
        radios.signal(&removed).unwrap();
        assert_eq!(radios.state(), LinkState::Off);
        // another signal leaves it alone
        let other = Message::signal("/org/bluez/hci0", "org.bluez.Other1", "Ping")
            .unwrap()
            .build(&())
            .unwrap();
        radios.signal(&other).unwrap();
        assert_eq!(radios.state(), LinkState::Off);
    }
}
