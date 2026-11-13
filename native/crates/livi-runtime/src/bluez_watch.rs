//! The helper's profiles (iAP2, HFP, Android Auto, the media player) live in bluetoothd. When bluetoothd starts again,
//! after a crash or by hand, they are gone: a phone connects and nothing answers it. The helper then ends, and LIVI's
//! core starts it again (`HelperSupervisor`), which registers them anew.

use futures_util::StreamExt;
use zbus::message::Type;
use zbus::{Connection, MatchRule, MessageStream};

const BLUEZ: &str = "org.bluez";
const DBUS: &str = "org.freedesktop.DBus";

/// Whether a change of org.bluez's owner is a bluetoothd that the helper's profiles are not registered with
pub fn new_bluez(old: &str, new: &str) -> bool {
    !new.is_empty() && old != new
}

/// Returns when another bluetoothd owns org.bluez than the one the helper registered with
pub async fn until_bluez_restarts(conn: &Connection) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(DBUS)?
        .interface(DBUS)?
        .member("NameOwnerChanged")?
        .add_arg(BLUEZ)?
        .build();
    let mut owners = MessageStream::for_match_rule(rule, conn, None).await?;
    while let Some(msg) = owners.next().await {
        let (_, old, new): (String, String, String) = msg?.body().deserialize()?;
        if new.is_empty() {
            println!("[helperd] BlueZ gone");
        } else if new_bluez(&old, &new) {
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::new_bluez;

    #[test]
    fn a_new_owner_is_a_new_bluez() {
        // a crash: first the old owner goes, then a new one comes
        assert!(!new_bluez(":1.12", ""));
        assert!(new_bluez("", ":1.60"));
        // handed over directly
        assert!(new_bluez(":1.12", ":1.60"));
        assert!(!new_bluez(":1.12", ":1.12"));
    }
}
