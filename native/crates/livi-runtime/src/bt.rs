use std::collections::HashMap;
use std::error::Error;
use std::os::fd::OwnedFd;

use crate::bluez_restart::{Part, Registry, Restarts};
use crate::shared_sock::Track;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::message::Type;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

pub use crate::bluetoothd::Wireless;

pub const AA_UUID: &str = "4de17a00-52cb-11e6-bdf4-0800200c9a66";
pub const AA_CHANNEL: u16 = 8;
const AA_PATH: &str = "/livi/aa/profile";

const AA_RECORD: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<record>
  <attribute id="0x0001"><sequence><uuid value="4de17a00-52cb-11e6-bdf4-0800200c9a66" /></sequence></attribute>
  <attribute id="0x0004"><sequence>
    <sequence><uuid value="0x0100" /></sequence>
    <sequence><uuid value="0x0003" /><uint8 value="0x08" /></sequence>
  </sequence></attribute>
  <attribute id="0x0005"><sequence><uuid value="0x1002" /></sequence></attribute>
  <attribute id="0x0100"><text value="Android Auto Wireless" /></attribute>
</record>
"#;

pub const IAP_SERVER_UUID: &str = "00000000-deca-fade-deca-deafdecacaff";
pub const IAP_CLIENT_UUID: &str = "00000000-deca-fade-deca-deafdecacafe";
pub const CARPLAY_SERVICE_UUID: &str = "ec884348-cd41-40a2-9727-575d50bf1fd3";
pub const IAP_CHANNEL: u16 = 3;

const ADAPTER_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
pub(crate) const ADAPTER1: &str = "org.bluez.Adapter1";
pub(crate) const MEDIA1: &str = "org.bluez.Media1";
pub(crate) const ADVERTISING1: &str = "org.bluez.LEAdvertisingManager1";
const BUSY_AGAIN: std::time::Duration = std::time::Duration::from_millis(200);
const LET_GO_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

const IAP_SERVER_PATH: &str = "/livi/cp/iap_server";
const IAP_CLIENT_PATH: &str = "/livi/cp/iap_client";
const CARPLAY_PATH: &str = "/livi/cp/carplay";
const AGENT_PATH: &str = "/livi/cp/agent";

const IAP_RECORD: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<record>
    <attribute id="0x0001"><sequence><uuid value="00000000-deca-fade-deca-deafdecacaff" /></sequence></attribute>
    <attribute id="0x0002"><uint32 value="0x00000000" /></attribute>
    <attribute id="0x0004"><sequence>
        <sequence><uuid value="0x0100" /></sequence>
        <sequence><uuid value="0x0003" /><uint8 value="0x03" /></sequence>
    </sequence></attribute>
    <attribute id="0x0005"><sequence><uuid value="0x1002" /></sequence></attribute>
    <attribute id="0x0008"><uint8 value="0xff" /></attribute>
    <attribute id="0x0009"><sequence><sequence><uuid value="0x1101" /><uint16 value="0x0100" /></sequence></sequence></attribute>
    <attribute id="0x0100"><text value="Wireless iAP" /></attribute>
</record>
"#;

const CARPLAY_RECORD: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<record>
    <attribute id="0x0001"><sequence><uuid value="ec884348-cd41-40a2-9727-575d50bf1fd3" /></sequence></attribute>
    <attribute id="0x0002"><uint32 value="0x00000000" /></attribute>
    <attribute id="0x0004"><sequence>
        <sequence><uuid value="0x0100" /></sequence>
        <sequence><uuid value="0x0003" /><uint8 value="0x04" /></sequence>
    </sequence></attribute>
    <attribute id="0x0005"><sequence><uuid value="0x1002" /></sequence></attribute>
    <attribute id="0x0008"><uint8 value="0xff" /></attribute>
    <attribute id="0x0009"><sequence><sequence><uuid value="0x1101" /><uint16 value="0x0100" /></sequence></sequence></attribute>
    <attribute id="0x0100"><text value="CarPlay" /></attribute>
</record>
"#;

pub struct IncomingConn {
    pub fd: OwnedFd,
    pub peer_mac: String,
}

struct Profile {
    tx: mpsc::UnboundedSender<IncomingConn>,
    adapter: String,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl Profile {
    async fn new_connection(
        &self,
        device: ObjectPath<'_>,
        fd: zbus::zvariant::OwnedFd,
        _options: HashMap<String, OwnedValue>,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<()> {
        refuse_other_adapter(device.as_str(), &self.adapter)?;
        trust(conn, device.as_str()).await;
        let peer_mac = mac_from_device_path(device.as_str());
        let fd = OwnedFd::from(fd);
        let _ = self.tx.send(IncomingConn { fd, peer_mac });
        Ok(())
    }

    fn request_disconnection(&self, _device: ObjectPath<'_>) {}

    fn release(&self) {}
}

struct Agent {
    adapter: String,
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}
    fn authorize_service(&self, device: ObjectPath<'_>, _uuid: String) -> zbus::fdo::Result<()> {
        refuse_other_adapter(device.as_str(), &self.adapter)
    }
    fn request_pin_code(&self, device: ObjectPath<'_>) -> zbus::fdo::Result<String> {
        refuse_other_adapter(device.as_str(), &self.adapter)?;
        Ok("0000".into())
    }
    fn request_passkey(&self, device: ObjectPath<'_>) -> zbus::fdo::Result<u32> {
        refuse_other_adapter(device.as_str(), &self.adapter)?;
        Ok(0)
    }
    fn display_passkey(&self, _device: ObjectPath<'_>, _passkey: u32, _entered: u16) {}
    fn display_pin_code(&self, _device: ObjectPath<'_>, _pincode: String) {}
    fn request_confirmation(&self, device: ObjectPath<'_>, _passkey: u32) -> zbus::fdo::Result<()> {
        refuse_other_adapter(device.as_str(), &self.adapter)
    }
    fn request_authorization(&self, device: ObjectPath<'_>) -> zbus::fdo::Result<()> {
        refuse_other_adapter(device.as_str(), &self.adapter)
    }
    fn cancel(&self) {}
}

#[derive(Clone, Debug, PartialEq)]
struct Found {
    alias: String,
    discoverable: bool,
    pairable: bool,
    discoverable_timeout: u32,
    pairable_timeout: u32,
}

/// Who answers pairing requests on LIVI's controller. By default LIVI's own agent, which accepts every pairing;
/// `LIVI_BT_AGENT=0` leaves pairing to another program on the system (e.g. one that asks the user and confirms the code):
/// LIVI then registers no agent and leaves the controller's `Pairable` alone, also when it lets the controller go.
/// Discovery stays LIVI's, as the wireless connections need it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Pairing {
    #[default]
    Livi,
    Other,
}

impl Pairing {
    /// From the value of `LIVI_BT_AGENT`: only "0" leaves pairing to another program
    pub fn from_env(value: Option<&str>) -> Self {
        if value.map(str::trim) == Some("0") {
            Pairing::Other
        } else {
            Pairing::Livi
        }
    }

    /// What `let_go` puts back on a controller; `ours` is LIVI's own. A timeout before its switch: BlueZ starts the
    /// timer when the switch goes on
    fn put_back(self, ours: bool) -> &'static [&'static str] {
        match (self, ours) {
            (Pairing::Other, true) => &["Alias", "DiscoverableTimeout", "Discoverable"],
            (Pairing::Livi, true) => &[
                "Alias",
                "DiscoverableTimeout",
                "Discoverable",
                "PairableTimeout",
                "Pairable",
            ],
            (_, false) => &["Alias", "DiscoverableTimeout", "Discoverable", "Pairable"],
        }
    }
}

/// How the helper sets up LIVI's controller
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    pub adapter: String,
    pub alias: String,
    pub wireless: Wireless,
    pub pairing: Pairing,
    /// LIVI_BT_PAIRING_SECONDS: new devices can find and pair with LIVI this long after the start, 0 always; paired
    /// devices connect regardless. BlueZ ends both itself (`DiscoverableTimeout`, `PairableTimeout`); the kernel takes
    /// at most 65535 s for discovery
    pub pairing_seconds: u32,
}

/// The settings of LIVI's controller in the order they are made; `rfkill unblock` comes before `Powered`. The timeouts
/// come before their switches: BlueZ starts the timer when the switch goes on. With pairing left to another program
/// (`Pairing::Other`) nothing about pairing
fn controller_settings(setup: &Setup) -> Vec<(&'static str, Value<'static>)> {
    let seconds = setup.pairing_seconds.min(u32::from(u16::MAX));
    let mut settings = vec![("Alias", Value::from(setup.alias.clone()))];
    if setup.wireless.any() {
        settings.push(("DiscoverableTimeout", Value::from(seconds)));
    }
    settings.push(("Powered", Value::from(true)));
    settings.push(("Discoverable", Value::from(setup.wireless.any())));
    if setup.pairing == Pairing::Livi {
        settings.push(("PairableTimeout", Value::from(seconds)));
        settings.push(("Pairable", Value::from(true)));
    }
    settings
}

#[derive(Clone, Default)]
pub struct Held(Vec<(String, Found)>, Pairing);

fn to_put_back(found: Found, alias: &str) -> Found {
    if found.alias == alias {
        Found {
            alias: String::new(),
            discoverable: false,
            ..found
        }
    } else {
        found
    }
}

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

async fn managed_objects(conn: &Connection) -> Result<Objects, Box<dyn Error>> {
    let reply = conn
        .call_method(
            Some("org.bluez"),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .await?;
    Ok(reply.body().deserialize()?)
}

async fn controllers(conn: &Connection) -> Result<Vec<(String, Found)>, Box<dyn Error>> {
    let objects = managed_objects(conn).await?;
    let mut out = Vec::new();
    for (path, interfaces) in objects {
        let Some(props) = interfaces.get("org.bluez.Adapter1") else {
            continue;
        };
        let Some(name) = path.as_str().strip_prefix("/org/bluez/") else {
            continue;
        };
        let value = |key: &str| props.get(key).and_then(|v| v.try_clone().ok());
        out.push((
            name.to_string(),
            Found {
                alias: value("Alias")
                    .and_then(|v| String::try_from(v).ok())
                    .unwrap_or_default(),
                discoverable: value("Discoverable").and_then(|v| bool::try_from(v).ok())
                    == Some(true),
                pairable: value("Pairable").and_then(|v| bool::try_from(v).ok()) == Some(true),
                discoverable_timeout: value("DiscoverableTimeout")
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(0),
                pairable_timeout: value("PairableTimeout")
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(0),
            },
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

async fn hold(conn: &Connection, adapter: &str, alias: &str, pairing: Pairing) -> Held {
    let found = match controllers(conn).await {
        Ok(found) => found,
        Err(e) => {
            eprintln!("[bt] the controllers could not be read, nothing will be put back: {e}");
            return Held(Vec::new(), pairing);
        }
    };
    let held = Held(
        found
            .into_iter()
            .map(|(name, found)| (name, to_put_back(found, alias)))
            .collect(),
        pairing,
    );
    hold_others(conn, adapter, &held).await;
    held
}

/// The other controllers neither carry LIVI's name nor take new devices while LIVI runs
pub(crate) async fn hold_others(conn: &Connection, adapter: &str, held: &Held) {
    for (name, back) in &held.0 {
        if name == adapter {
            continue;
        }
        let path = format!("/org/bluez/{name}");
        if back.alias.is_empty()
            && let Err(e) = set_prop(conn, &path, "Alias", Value::from("")).await
        {
            eprintln!("[bt] {name} keeps LIVI's name: {e}");
        }
        for prop in ["Discoverable", "Pairable"] {
            if let Err(e) = set_prop(conn, &path, prop, Value::from(false)).await {
                eprintln!("[bt] {name} could not be set {prop}=false: {e}");
            }
        }
    }
}

pub async fn let_go(conn: &Connection, adapter: &str, held: &Held) {
    stop_ble_ad(conn, adapter).await;
    for (name, back) in &held.0 {
        let path = format!("/org/bluez/{name}");
        for &prop in held.1.put_back(name == adapter) {
            let value = match prop {
                "Alias" => Value::from(back.alias.as_str()),
                "DiscoverableTimeout" => Value::from(back.discoverable_timeout),
                "Discoverable" => Value::from(back.discoverable),
                "PairableTimeout" => Value::from(back.pairable_timeout),
                _ => Value::from(back.pairable),
            };
            if let Err(e) = set_prop_within(conn, &path, prop, value, LET_GO_WAIT).await {
                eprintln!("[bt] {name}: {prop} could not be put back: {e}");
            }
        }
    }
}

fn refuse_other_adapter(device: &str, adapter: &str) -> zbus::fdo::Result<()> {
    if device.starts_with(&format!("/org/bluez/{adapter}/")) {
        return Ok(());
    }
    println!(
        "[bt] {} came in over another controller than {adapter}, turned away",
        mac_from_device_path(device)
    );
    Err(zbus::fdo::Error::AccessDenied(format!(
        "LIVI listens on {adapter} only"
    )))
}

fn mac_from_device_path(path: &str) -> String {
    let tail = path.rsplit("/dev_").next().unwrap_or("");
    let parts: Vec<&str> = tail.split('_').collect();
    if parts.len() != 6 {
        return String::new();
    }
    parts.join(":").to_uppercase()
}

async fn register_profile(
    conn: &Connection,
    path: &str,
    uuid: &str,
    options: HashMap<&str, Value<'_>>,
) -> Result<(), Box<dyn Error>> {
    let object_path = ObjectPath::try_from(path)?;
    conn.call_method(
        Some("org.bluez"),
        "/org/bluez",
        Some("org.bluez.ProfileManager1"),
        "RegisterProfile",
        &(object_path, uuid, options),
    )
    .await?;
    Ok(())
}

/// Registers LIVI with the system's bluetoothd and sets up its controller. The registry carries what it registered, and
/// what the later `start_*` add, to a bluetoothd that starts anew (`Registry::follow`)
pub async fn start(
    setup: &Setup,
) -> Result<
    (
        Connection,
        mpsc::UnboundedReceiver<IncomingConn>,
        Held,
        Registry,
    ),
    Box<dyn Error>,
> {
    start_on(Connection::system().await?, setup).await
}

/// `start` on a connection of the caller's (the tests' stand-in for bluetoothd)
pub async fn start_on(
    conn: Connection,
    setup: &Setup,
) -> Result<
    (
        Connection,
        mpsc::UnboundedReceiver<IncomingConn>,
        Held,
        Registry,
    ),
    Box<dyn Error>,
> {
    let (adapter, alias, pairing) = (setup.adapter.as_str(), setup.alias.as_str(), setup.pairing);
    // followed before the first registration: a bluetoothd that comes after it is told everything again
    let restarts = Restarts::follow(&conn).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let mut parts = Vec::new();

    if pairing == Pairing::Livi {
        conn.object_server()
            .at(
                AGENT_PATH,
                Agent {
                    adapter: adapter.to_string(),
                },
            )
            .await?;
    }
    if setup.wireless.carplay {
        export_carplay(&conn, adapter, tx).await?;
        register_carplay(&conn).await?;
        parts.push(Part::CarPlay);
    }

    if pairing == Pairing::Livi {
        register_agent(&conn).await?;
        parts.push(Part::Agent);
    } else {
        println!(
            "[bt] pairing is left to another program (LIVI_BT_AGENT=0): no agent, Pairable untouched"
        );
    }

    let path = format!("/org/bluez/{adapter}");
    adapter_ready(&conn, &path, &[ADAPTER1], Some(ADAPTER_WAIT)).await?;
    let held = hold(&conn, adapter, alias, pairing).await;
    configure(&conn, setup).await?;

    let registry = Registry::new(conn.clone(), setup.clone(), held.clone(), parts, restarts);
    Ok((conn, rx, held, registry))
}

async fn register_agent(conn: &Connection) -> Result<(), Box<dyn Error>> {
    conn.call_method(
        Some("org.bluez"),
        "/org/bluez",
        Some("org.bluez.AgentManager1"),
        "RegisterAgent",
        &(ObjectPath::try_from(AGENT_PATH)?, "KeyboardDisplay"),
    )
    .await?;
    conn.call_method(
        Some("org.bluez"),
        "/org/bluez",
        Some("org.bluez.AgentManager1"),
        "RequestDefaultAgent",
        &(ObjectPath::try_from(AGENT_PATH)?,),
    )
    .await?;
    Ok(())
}

/// Registers a part again with a bluetoothd that started anew (`Registry::follow`)
pub(crate) async fn register_part(
    conn: &Connection,
    adapter: &str,
    part: &Part,
) -> Result<(), Box<dyn Error>> {
    match part {
        Part::Agent => register_agent(conn).await,
        Part::CarPlay => register_carplay(conn).await,
        Part::Hfp => register_hfp(conn).await,
        Part::AndroidAuto => register_aa(conn).await,
        Part::Advertisement => register_ble_ad(conn, adapter).await,
        Part::Player { status, track } => register_player(conn, adapter, status, track).await,
    }
}

/// Sets up LIVI's controller again for a bluetoothd that started anew: the other controllers as at the start, then LIVI's
pub(crate) async fn configure_again(
    conn: &Connection,
    setup: &Setup,
    held: &Held,
) -> Result<(), Box<dyn Error>> {
    hold_others(conn, &setup.adapter, held).await;
    configure(conn, setup).await
}

/// Sets up LIVI's controller (`controller_settings`)
async fn configure(conn: &Connection, setup: &Setup) -> Result<(), Box<dyn Error>> {
    let path = format!("/org/bluez/{}", setup.adapter);
    for (name, value) in controller_settings(setup) {
        if name == "Powered" {
            let _ = std::process::Command::new(crate::sys::tool("rfkill"))
                .args(["unblock", "bluetooth"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        set_prop(conn, &path, name, value).await?;
    }
    if setup.pairing_seconds > 0 && setup.wireless.any() {
        println!(
            "[bt] new devices find and pair with LIVI for {} s, BlueZ ends it",
            setup.pairing_seconds.min(u32::from(u16::MAX))
        );
    }
    Ok(())
}

async fn export_carplay(
    conn: &Connection,
    adapter: &str,
    tx: mpsc::UnboundedSender<IncomingConn>,
) -> Result<(), Box<dyn Error>> {
    let ours = || adapter.to_string();
    conn.object_server()
        .at(
            IAP_SERVER_PATH,
            Profile {
                tx: tx.clone(),
                adapter: ours(),
            },
        )
        .await?;
    conn.object_server()
        .at(
            IAP_CLIENT_PATH,
            Profile {
                tx: tx.clone(),
                adapter: ours(),
            },
        )
        .await?;
    conn.object_server()
        .at(
            CARPLAY_PATH,
            Profile {
                tx,
                adapter: ours(),
            },
        )
        .await?;
    Ok(())
}

/// The iAP2 server profile (RFCOMM channel 3), which a CarPlay phone needs; the client profile and the record that
/// announces CarPlay only with a line when BlueZ refuses them
async fn register_carplay(conn: &Connection) -> Result<(), Box<dyn Error>> {
    let mut iap_opts: HashMap<&str, Value> = HashMap::new();
    iap_opts.insert("Role", Value::from("server"));
    iap_opts.insert("Channel", Value::from(IAP_CHANNEL));
    iap_opts.insert("ServiceRecord", Value::from(IAP_RECORD));
    iap_opts.insert("RequireAuthentication", Value::from(false));
    iap_opts.insert("RequireAuthorization", Value::from(false));
    register_profile(conn, IAP_SERVER_PATH, IAP_SERVER_UUID, iap_opts).await?;

    let mut client_opts: HashMap<&str, Value> = HashMap::new();
    client_opts.insert("Role", Value::from("client"));
    client_opts.insert("AutoConnect", Value::from(true));
    client_opts.insert("RequireAuthentication", Value::from(true));
    if let Err(e) = register_profile(conn, IAP_CLIENT_PATH, IAP_CLIENT_UUID, client_opts).await {
        eprintln!("[cp] could not register iAP client profile: {e}");
    }

    let mut cp_opts: HashMap<&str, Value> = HashMap::new();
    cp_opts.insert("Role", Value::from("server"));
    cp_opts.insert("ServiceRecord", Value::from(CARPLAY_RECORD));
    cp_opts.insert("RequireAuthentication", Value::from(false));
    cp_opts.insert("RequireAuthorization", Value::from(false));
    if let Err(e) = register_profile(conn, CARPLAY_PATH, CARPLAY_SERVICE_UUID, cp_opts).await {
        eprintln!("[cp] could not publish CarPlay service UUID: {e}");
    }

    Ok(())
}

/// Waits until bluetoothd shows the controller at `path` with all of `needs`, at most `patience` (None: as long as it
/// takes). ObjectManager's InterfacesAdded, followed before a look at what is there already, so that nothing slips
/// between the two
pub(crate) async fn adapter_ready(
    conn: &Connection,
    path: &str,
    needs: &[&str],
    patience: Option<std::time::Duration>,
) -> Result<(), Box<dyn Error>> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.bluez")?
        .interface("org.freedesktop.DBus.ObjectManager")?
        .member("InterfacesAdded")?
        .build();
    let mut added = MessageStream::for_match_rule(rule, conn, None).await?;
    let objects = managed_objects(conn).await?;
    let there = objects
        .iter()
        .find(|(p, _)| p.as_str() == path)
        .map(|(_, i)| i);
    let mut missing = still_missing(needs, there.map(|i| i.keys().map(String::as_str).collect()));
    if !missing.is_empty() {
        println!(
            "[bt] waiting for BlueZ to show {path} ({})",
            missing.join(", ")
        );
    }
    let deadline = patience.map(|p| tokio::time::Instant::now() + p);
    while !missing.is_empty() {
        let next = async {
            match deadline {
                Some(deadline) => tokio::time::timeout_at(deadline, added.next()).await,
                None => Ok(added.next().await),
            }
        };
        let msg = match next.await {
            Ok(Some(msg)) => msg?,
            Ok(None) => return Err("the system bus closed".into()),
            Err(_) => {
                return Err(
                    format!("BlueZ never published {path} with {}", missing.join(", ")).into(),
                );
            }
        };
        let (object, interfaces): (
            OwnedObjectPath,
            HashMap<String, HashMap<String, OwnedValue>>,
        ) = msg.body().deserialize()?;
        if object.as_str() == path {
            missing = still_missing(
                &missing,
                Some(interfaces.keys().map(String::as_str).collect()),
            );
        }
    }
    Ok(())
}

/// Which of `needs` an object does not have (all, when it is not there)
fn still_missing<'a>(needs: &[&'a str], has: Option<Vec<&str>>) -> Vec<&'a str> {
    let has = has.unwrap_or_default();
    needs.iter().copied().filter(|n| !has.contains(n)).collect()
}

async fn set_prop(
    conn: &Connection,
    path: &str,
    name: &str,
    value: Value<'_>,
) -> Result<(), Box<dyn Error>> {
    set_prop_within(conn, path, name, value, ADAPTER_WAIT).await
}

async fn set_prop_within(
    conn: &Connection,
    path: &str,
    name: &str,
    value: Value<'_>,
    patience: std::time::Duration,
) -> Result<(), Box<dyn Error>> {
    let deadline = std::time::Instant::now() + patience;
    loop {
        let asked = conn
            .call_method(
                Some("org.bluez"),
                path,
                Some("org.freedesktop.DBus.Properties"),
                "Set",
                &("org.bluez.Adapter1", name, &value),
            )
            .await;
        match asked {
            Ok(_) => return Ok(()),
            Err(e) if busy(&e) && std::time::Instant::now() < deadline => {
                println!("[bt] {path} is still busy, offering {name} again");
                tokio::time::sleep(BUSY_AGAIN).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

async fn trust(conn: &Connection, device: &str) {
    let set = conn
        .call_method(
            Some("org.bluez"),
            device,
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &("org.bluez.Device1", "Trusted", Value::from(true)),
        )
        .await;
    if let Err(e) = set {
        println!("[bt] {device}: not marked trusted: {e}");
    }
}

fn busy(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, message, _) => {
            busy_answer(name.as_str(), message.as_deref())
        }
        _ => false,
    }
}

fn busy_answer(name: &str, message: Option<&str>) -> bool {
    name == "org.bluez.Error.Busy" || (name == "org.bluez.Error.Failed" && message == Some("Busy"))
}

pub async fn start_aa(
    registry: &Registry,
) -> Result<mpsc::UnboundedReceiver<IncomingConn>, Box<dyn Error>> {
    let conn = registry.conn();
    let (tx, rx) = mpsc::unbounded_channel();
    conn.object_server()
        .at(
            AA_PATH,
            Profile {
                tx,
                adapter: registry.adapter().to_string(),
            },
        )
        .await?;
    register_aa(conn).await?;
    registry.add(Part::AndroidAuto);
    println!("[aa] profile registered (RFCOMM ch {AA_CHANNEL})");
    Ok(rx)
}

async fn register_aa(conn: &Connection) -> Result<(), Box<dyn Error>> {
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("Role", Value::from("server"));
    opts.insert("Channel", Value::from(AA_CHANNEL));
    opts.insert("ServiceRecord", Value::from(AA_RECORD));
    opts.insert("RequireAuthentication", Value::from(false));
    opts.insert("RequireAuthorization", Value::from(false));
    register_profile(conn, AA_PATH, AA_UUID, opts).await
}

pub const HFP_HF_UUID: &str = "0000111e-0000-1000-8000-00805f9b34fb";
const HFP_PATH: &str = "/livi/bt/hfp";
const BLE_AD_PATH: &str = "/livi/bt/ble";
const PLAYER_PATH: &str = "/livi/bt/player";

struct HfpProfile {
    hfp: crate::hfp::Hfp,
    adapter: String,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl HfpProfile {
    fn new_connection(
        &self,
        device: ObjectPath<'_>,
        fd: zbus::zvariant::OwnedFd,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        refuse_other_adapter(device.as_str(), &self.adapter)?;
        let mac = mac_from_device_path(device.as_str());
        println!("[hfp] connection from {mac}");
        self.hfp.accept(OwnedFd::from(fd), mac);
        Ok(())
    }

    fn request_disconnection(&self, _device: ObjectPath<'_>) {}

    fn release(&self) {}
}

/// The hands-free profile, if no other program has it (the audio daemon, oFono): only then does a bluetoothd that
/// starts anew get it from LIVI again, so that the program that had it at the start keeps it
pub async fn start_hfp(registry: &Registry, hfp: crate::hfp::Hfp) -> Result<(), Box<dyn Error>> {
    let conn = registry.conn();
    conn.object_server()
        .at(
            HFP_PATH,
            HfpProfile {
                hfp,
                adapter: registry.adapter().to_string(),
            },
        )
        .await?;
    match register_hfp(conn).await {
        Ok(()) => {
            registry.add(Part::Hfp);
            println!("[hfp] HF profile registered")
        }
        Err(e) => println!("[hfp] HF profile held by the audio daemon ({e})"),
    }
    Ok(())
}

async fn register_hfp(conn: &Connection) -> Result<(), Box<dyn Error>> {
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("Name", Value::from("HFP Hands-Free"));
    opts.insert("Role", Value::from("client"));
    opts.insert("RequireAuthentication", Value::from(true));
    opts.insert("RequireAuthorization", Value::from(false));
    opts.insert("Features", Value::from(0x009cu16));
    opts.insert("Version", Value::from(0x0108u16));
    register_profile(conn, HFP_PATH, HFP_HF_UUID, opts).await
}

struct BleAd {
    name: String,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl BleAd {
    fn release(&self) {}

    #[zbus(property, name = "Type")]
    fn ad_type(&self) -> String {
        "peripheral".into()
    }

    #[zbus(property, name = "ServiceUUIDs")]
    fn service_uuids(&self) -> Vec<String> {
        vec![AA_UUID.into()]
    }

    #[zbus(property, name = "LocalName")]
    fn local_name(&self) -> String {
        self.name.clone()
    }
}

pub async fn start_ble_ad(registry: &Registry, name: &str) -> Result<(), Box<dyn Error>> {
    let conn = registry.conn();
    conn.object_server()
        .at(BLE_AD_PATH, BleAd { name: name.into() })
        .await?;
    register_ble_ad(conn, registry.adapter()).await?;
    registry.add(Part::Advertisement);
    println!("[aa] BLE advertisement registered");
    Ok(())
}

async fn register_ble_ad(conn: &Connection, adapter: &str) -> Result<(), Box<dyn Error>> {
    let path = format!("/org/bluez/{adapter}");
    let opts: HashMap<&str, Value> = HashMap::new();
    conn.call_method(
        Some("org.bluez"),
        path.as_str(),
        Some(ADVERTISING1),
        "RegisterAdvertisement",
        &(ObjectPath::try_from(BLE_AD_PATH)?, opts),
    )
    .await?;
    Ok(())
}

async fn stop_ble_ad(conn: &Connection, adapter: &str) {
    let Ok(ad) = ObjectPath::try_from(BLE_AD_PATH) else {
        return;
    };
    let _ = conn
        .call_method(
            Some("org.bluez"),
            format!("/org/bluez/{adapter}").as_str(),
            Some("org.bluez.LEAdvertisingManager1"),
            "UnregisterAdvertisement",
            &(ad,),
        )
        .await;
}

struct MprisRoot;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl MprisRoot {
    fn raise(&self) {}
    fn quit(&self) {}

    #[zbus(property, name = "CanQuit")]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property, name = "CanRaise")]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property, name = "HasTrackList")]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property, name = "Identity")]
    fn identity(&self) -> String {
        "LIVI".into()
    }

    #[zbus(property, name = "SupportedUriSchemes")]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec![]
    }

    #[zbus(property, name = "SupportedMimeTypes")]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![]
    }
}

pub struct MprisPlayer {
    events: crate::livi_sock::Broadcaster,
    status: std::sync::Arc<std::sync::Mutex<String>>,
    track: std::sync::Arc<std::sync::Mutex<Track>>,
}

/// BlueZ gives these to the other side over AVRCP: a car's head unit shows them as its Bluetooth source.
fn track_metadata(track: &Track) -> HashMap<String, OwnedValue> {
    let mut fields = HashMap::new();
    let mut put = |key: &str, value: Value<'_>| {
        if let Ok(value) = OwnedValue::try_from(value) {
            fields.insert(key.to_string(), value);
        }
    };
    if !track.title.is_empty() {
        put("xesam:title", Value::from(track.title.as_str()));
    }
    if !track.artist.is_empty() {
        put("xesam:artist", Value::from(vec![track.artist.as_str()]));
    }
    if !track.album.is_empty() {
        put("xesam:album", Value::from(track.album.as_str()));
    }
    if track.duration_ms > 0 {
        put("mpris:length", Value::from(track.duration_ms as i64 * 1000));
    }
    fields
}

#[derive(Clone)]
pub struct MediaPlayerHandle {
    conn: Connection,
    status: std::sync::Arc<std::sync::Mutex<String>>,
    track: std::sync::Arc<std::sync::Mutex<Track>>,
}

impl MediaPlayerHandle {
    pub async fn set_track(&self, track: Track) {
        {
            let mut now = self.track.lock().unwrap();
            if *now == track {
                return;
            }
            *now = track;
        }
        if let Ok(iface) = self
            .conn
            .object_server()
            .interface::<_, MprisPlayer>(PLAYER_PATH)
            .await
        {
            let _ = iface
                .get()
                .await
                .metadata_changed(iface.signal_emitter())
                .await;
        }
    }

    pub async fn set_status(&self, status: &str) {
        {
            let mut s = self.status.lock().unwrap();
            if *s == status {
                return;
            }
            *s = status.to_string();
        }
        println!("[aa] avrcp playback status -> {status}");
        if let Ok(iface) = self
            .conn
            .object_server()
            .interface::<_, MprisPlayer>(PLAYER_PATH)
            .await
        {
            let _ = iface
                .get()
                .await
                .playback_status_changed(iface.signal_emitter())
                .await;
        }
    }
}

impl MprisPlayer {
    fn emit(&self, command: &str) {
        self.events
            .push_json(format!("{{\"event\":\"input\",\"command\":\"{command}\"}}"));
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    fn play(&self) {
        self.emit("play");
    }
    fn pause(&self) {
        self.emit("pause");
    }
    fn play_pause(&self) {
        self.emit("playPause");
    }
    fn stop(&self) {
        self.emit("stop");
    }
    fn next(&self) {
        self.emit("next");
    }
    fn previous(&self) {
        self.emit("previous");
    }
    fn seek(&self, offset: i64) {
        self.emit(if offset > 0 { "fastForward" } else { "rewind" });
    }
    fn set_position(&self, _track: ObjectPath<'_>, _position: i64) {}
    fn open_uri(&self, _uri: String) {}

    #[zbus(property, name = "PlaybackStatus")]
    fn playback_status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    #[zbus(property, name = "LoopStatus")]
    fn loop_status(&self) -> String {
        "None".into()
    }

    #[zbus(property, name = "Rate")]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property, name = "Shuffle")]
    fn shuffle(&self) -> bool {
        false
    }

    #[zbus(property, name = "Metadata")]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        track_metadata(&self.track.lock().unwrap())
    }

    #[zbus(property, name = "Volume")]
    fn volume(&self) -> f64 {
        1.0
    }

    #[zbus(property, name = "Position")]
    fn position(&self) -> i64 {
        0
    }

    #[zbus(property, name = "MinimumRate")]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property, name = "MaximumRate")]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property, name = "CanGoNext")]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property, name = "CanGoPrevious")]
    fn can_go_previous(&self) -> bool {
        true
    }

    #[zbus(property, name = "CanPlay")]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property, name = "CanPause")]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property, name = "CanSeek")]
    fn can_seek(&self) -> bool {
        false
    }

    #[zbus(property, name = "CanControl")]
    fn can_control(&self) -> bool {
        true
    }
}

pub async fn start_media_player(
    registry: &Registry,
    events: crate::livi_sock::Broadcaster,
) -> Result<MediaPlayerHandle, Box<dyn Error>> {
    let conn = registry.conn();
    let status = std::sync::Arc::new(std::sync::Mutex::new("Playing".to_string()));
    let track = std::sync::Arc::new(std::sync::Mutex::new(Track::default()));
    conn.object_server().at(PLAYER_PATH, MprisRoot).await?;
    conn.object_server()
        .at(
            PLAYER_PATH,
            MprisPlayer {
                events,
                status: status.clone(),
                track: track.clone(),
            },
        )
        .await?;
    register_player(conn, registry.adapter(), &status, &track).await?;
    registry.add(Part::Player {
        status: status.clone(),
        track: track.clone(),
    });
    println!("[aa] media player registered at {PLAYER_PATH}");
    Ok(MediaPlayerHandle {
        conn: conn.clone(),
        status,
        track,
    })
}

/// What BlueZ learns of the player when it is registered; later changes come as PropertiesChanged. A bluetoothd that
/// starts anew gets what plays now
fn player_properties(status: &str, track: &Track) -> HashMap<&'static str, Value<'static>> {
    let mut props: HashMap<&'static str, Value<'static>> = HashMap::new();
    props.insert("PlaybackStatus", Value::from(status.to_string()));
    props.insert("LoopStatus", Value::from("None"));
    props.insert("Rate", Value::from(1.0f64));
    props.insert("Shuffle", Value::from(false));
    props.insert("Volume", Value::from(1.0f64));
    props.insert("Position", Value::from(0i64));
    props.insert("MinimumRate", Value::from(1.0f64));
    props.insert("MaximumRate", Value::from(1.0f64));
    props.insert("CanGoNext", Value::from(true));
    props.insert("CanGoPrevious", Value::from(true));
    props.insert("CanPlay", Value::from(true));
    props.insert("CanPause", Value::from(true));
    props.insert("CanSeek", Value::from(false));
    props.insert("CanControl", Value::from(true));
    let metadata = track_metadata(track);
    if !metadata.is_empty() {
        props.insert("Metadata", Value::from(metadata));
    }
    props
}

async fn register_player(
    conn: &Connection,
    adapter: &str,
    status: &std::sync::Mutex<String>,
    track: &std::sync::Mutex<Track>,
) -> Result<(), Box<dyn Error>> {
    let props = {
        let (status, track) = (status.lock().unwrap(), track.lock().unwrap());
        player_properties(&status, &track)
    };
    let path = format!("/org/bluez/{adapter}");
    conn.call_method(
        Some("org.bluez"),
        path.as_str(),
        Some(MEDIA1),
        "RegisterPlayer",
        &(ObjectPath::try_from(PLAYER_PATH)?, props),
    )
    .await?;
    Ok(())
}

pub async fn adapter_address(conn: &Connection, adapter: &str) -> Result<[u8; 6], Box<dyn Error>> {
    let path = format!("/org/bluez/{adapter}");
    let reply = conn
        .call_method(
            Some("org.bluez"),
            path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.bluez.Adapter1", "Address"),
        )
        .await?;
    let value: OwnedValue = reply.body().deserialize()?;
    let addr = String::try_from(value)?;
    let mut mac = [0u8; 6];
    for (i, part) in addr.split(':').enumerate().take(6) {
        mac[i] = u8::from_str_radix(part, 16).unwrap_or(0);
    }
    Ok(mac)
}

pub fn drop_link(conn: &Connection, adapter: &str, mac: String) {
    let (conn, adapter) = (conn.clone(), adapter.to_string());
    tokio::spawn(async move {
        if let Err(e) = crate::livi_sock::device_disconnect(&conn, &adapter, &mac).await {
            eprintln!("[helperd] {mac} stays on bluetooth: {e}");
        }
    });
}

pub fn on_cable(
    state: std::sync::Arc<crate::state::HelperState>,
    conn: &Connection,
    adapter: &str,
) -> crate::bringup::OnCable {
    let (conn, adapter) = (conn.clone(), adapter.to_string());
    crate::bringup::OnCable(std::sync::Arc::new(move |mac: &str| {
        let cabled = state.carkit_claims(mac);
        if cabled {
            drop_link(&conn, &adapter, mac.to_string());
        }
        cabled
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_chosen_controller_is_answered() {
        let phone = "/org/bluez/hci2/dev_0C_6A_C4_4E_F3_2A";
        assert!(refuse_other_adapter(phone, "hci2").is_ok());
        assert!(refuse_other_adapter(phone, "hci0").is_err());
        assert!(refuse_other_adapter("/org/bluez/hci10/dev_0C_6A_C4_4E_F3_2A", "hci1").is_err());
    }

    fn found(alias: &str, discoverable: bool) -> Found {
        Found {
            alias: alias.into(),
            discoverable,
            pairable: true,
            discoverable_timeout: 180,
            pairable_timeout: 0,
        }
    }

    #[test]
    fn a_controller_goes_back_the_way_it_was_found() {
        assert_eq!(
            to_put_back(found("blacky", false), "LIVI blacky"),
            found("blacky", false)
        );
        assert_eq!(
            to_put_back(found("blacky #2", true), "LIVI blacky"),
            found("blacky #2", true)
        );
    }

    #[test]
    fn livis_name_left_on_a_controller_goes_back_to_the_systems() {
        let back = to_put_back(found("LIVI blacky", true), "LIVI blacky");
        assert_eq!(
            back,
            Found {
                alias: String::new(),
                discoverable: false,
                ..found("", true)
            }
        );
    }

    #[test]
    fn only_zero_leaves_pairing_to_another_program() {
        assert_eq!(Pairing::from_env(None), Pairing::Livi);
        assert_eq!(Pairing::from_env(Some("1")), Pairing::Livi);
        assert_eq!(Pairing::from_env(Some("")), Pairing::Livi);
        assert_eq!(Pairing::from_env(Some("0")), Pairing::Other);
        assert_eq!(Pairing::from_env(Some(" 0 ")), Pairing::Other);
        assert_eq!(Pairing::default(), Pairing::Livi);
    }

    fn setup(pairing: Pairing, wireless: bool, seconds: u32) -> Setup {
        Setup {
            adapter: "hci0".into(),
            alias: "LIVI".into(),
            wireless: Wireless {
                carplay: wireless,
                android_auto: false,
            },
            pairing,
            pairing_seconds: seconds,
        }
    }

    fn names(settings: &[(&'static str, Value<'static>)]) -> Vec<&'static str> {
        settings.iter().map(|(name, _)| *name).collect()
    }

    #[test]
    fn with_pairing_left_to_another_program_pairable_is_never_touched() {
        let other = controller_settings(&setup(Pairing::Other, true, 120));
        assert!(!names(&other).iter().any(|n| n.starts_with("Pairable")));
        assert!(Pairing::Livi.put_back(true).contains(&"Pairable"));
        assert!(Pairing::Livi.put_back(true).contains(&"PairableTimeout"));
        assert!(!Pairing::Other.put_back(true).contains(&"Pairable"));
        assert!(!Pairing::Other.put_back(true).contains(&"PairableTimeout"));
        // another controller LIVI held goes back as it was found, whoever pairs on LIVI's
        assert!(Pairing::Other.put_back(false).contains(&"Pairable"));
        assert!(Pairing::Other.put_back(true).contains(&"Discoverable"));
    }

    #[test]
    fn bluez_ends_the_pairing_window_its_timeouts_come_before_their_switches() {
        let livi = controller_settings(&setup(Pairing::Livi, true, 120));
        assert_eq!(
            names(&livi),
            [
                "Alias",
                "DiscoverableTimeout",
                "Powered",
                "Discoverable",
                "PairableTimeout",
                "Pairable"
            ]
        );
        let value = |name: &str| {
            livi.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(value("DiscoverableTimeout"), Some(Value::from(120u32)));
        assert_eq!(value("PairableTimeout"), Some(Value::from(120u32)));
        assert_eq!(value("Discoverable"), Some(Value::from(true)));
        // 0: open for good, as BlueZ takes a timeout of 0
        let open = controller_settings(&setup(Pairing::Livi, true, 0));
        assert!(open.contains(&("DiscoverableTimeout", Value::from(0u32))));
        assert!(open.contains(&("PairableTimeout", Value::from(0u32))));
        // the kernel takes 16 bits for discovery
        let long = controller_settings(&setup(Pairing::Livi, true, 100_000));
        assert!(long.contains(&("DiscoverableTimeout", Value::from(65_535u32))));
        // nothing wireless: not discoverable, no discovery timeout
        let wired = controller_settings(&setup(Pairing::Livi, false, 120));
        assert_eq!(
            names(&wired),
            [
                "Alias",
                "Powered",
                "Discoverable",
                "PairableTimeout",
                "Pairable"
            ]
        );
        assert!(wired.contains(&("Discoverable", Value::from(false))));
    }

    #[test]
    fn busy_comes_as_its_own_error_or_as_a_failed_mode_change() {
        assert!(busy_answer("org.bluez.Error.Busy", None));
        assert!(busy_answer("org.bluez.Error.Failed", Some("Busy")));
        assert!(!busy_answer("org.bluez.Error.Failed", Some("Not Powered")));
        assert!(!busy_answer("org.bluez.Error.Blocked", Some("Busy")));
    }
}

#[cfg(test)]
mod track_tests {
    use super::*;

    #[test]
    fn only_what_the_phone_told_goes_out() {
        let fields = track_metadata(&Track {
            title: "Song".into(),
            artist: "Band".into(),
            album: String::new(),
            duration_ms: 2000,
        });
        assert_eq!(fields.len(), 3);
        assert!(!fields.contains_key("xesam:album"));
        assert_eq!(i64::try_from(&fields["mpris:length"]).unwrap(), 2_000_000);
        assert!(track_metadata(&Track::default()).is_empty());
    }
}
