//! The phone's classic (BR/EDR) link after its `disableBluetooth`: CarPlay moved to Wi-Fi, the phone asks to be let go
//! over Bluetooth. `Device1.Disconnect` ends every link to it, the LE one of a notification service too. When it does not
//! answer or the phone stays on BR/EDR, the helper closes the classic link alone, once: BlueZ's
//! `org.bluez.Bearer.BREDR1.Disconnect` (BlueZ 5.86 on), else the kernel's management interface for that one bearer
//! (`btmgmt disconnect -t 0`, as BlueZ does it itself). LE stays.

use std::future::Future;
use std::time::Duration;

use tokio::process::Command;

use crate::state::HelperState;

/// How long `Device1.Disconnect` may take; LIVI waits 8 s for the whole answer
pub const DISCONNECT_WAIT: Duration = Duration::from_secs(4);
/// After the answer, the time for the other bearer to go down as well
const SETTLE: Duration = Duration::from_secs(1);
/// For the look at the classic link
const LOOK_WAIT: Duration = Duration::from_secs(1);
/// For closing it, of which BlueZ's bearer gets a part
const BREDR_WAIT: Duration = Duration::from_millis(1500);
const BEARER_WAIT: Duration = Duration::from_millis(500);

const BEARER_BREDR: &str = "org.bluez.Bearer.BREDR1";

/// What the helper asks of the system about one phone
pub trait Classic {
    /// `Device1.Disconnect`: every link to the phone
    fn disconnect(&self, mac: &str) -> impl Future<Output = Result<(), String>> + Send;
    /// Whether the phone is on BR/EDR; None: this system cannot tell
    fn bredr_up(&self, mac: &str) -> impl Future<Output = Option<bool>> + Send;
    /// Ends the BR/EDR link alone; Ok names the way it went
    fn disconnect_bredr(&self, mac: &str)
    -> impl Future<Output = Result<&'static str, String>> + Send;
}

/// The phone said `disableBluetooth`: every link goes, and if the classic one is still there after that (or
/// `Disconnect` did not answer), the helper's own iAP2 link to it and its BR/EDR link go once more, LE stays. One line
/// to the log when it comes to that
pub async fn after_disable_bluetooth<C: Classic>(
    c: &C,
    state: &HelperState,
    mac: &str,
) -> Result<(), String> {
    let first = match tokio::time::timeout(DISCONNECT_WAIT, c.disconnect(mac)).await {
        Ok(answer) => answer,
        Err(_) => Err(format!(
            "no answer within {} s",
            DISCONNECT_WAIT.as_secs()
        )),
    };
    if first.is_ok() {
        tokio::time::sleep(SETTLE).await;
    }
    let iap2 = state.bt_link_active(mac);
    let bredr = tokio::time::timeout(LOOK_WAIT, c.bredr_up(mac))
        .await
        .ok()
        .flatten();
    // the kernel's word when there is one (an iAP2 link of a gone BR/EDR is about to end); without it the helper's own
    // link, and a Disconnect that failed
    if !bredr.unwrap_or(iap2 || first.is_err()) {
        return Ok(());
    }
    let why = match &first {
        Ok(()) => "Disconnect answered".to_string(),
        Err(e) => format!("Disconnect: {e}"),
    };
    let seen = match (iap2, bredr) {
        (true, Some(true)) => "its iAP2 link and BR/EDR are up",
        (true, _) => "its iAP2 link is up",
        (false, _) => "BR/EDR is up",
    };
    let own = if iap2 && state.close_bt(mac, None) {
        "iAP2 link shut, "
    } else {
        ""
    };
    let closed = match tokio::time::timeout(BREDR_WAIT, c.disconnect_bredr(mac)).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "no answer within {} ms",
            BREDR_WAIT.as_millis()
        )),
    };
    match &closed {
        Ok(way) => println!(
            "[cp] disableBluetooth: {mac} kept its classic link ({why}, {seen}); {own}BR/EDR closed alone ({way}), LE stays"
        ),
        Err(e) => println!(
            "[cp] disableBluetooth: {mac} kept its classic link ({why}, {seen}); {own}BR/EDR not closed: {e}"
        ),
    }
    closed.map(drop)
}

/// The helper's BlueZ, and the kernel's management interface through BlueZ's `btmgmt`
pub struct BluezClassic<'a> {
    pub bus: &'a zbus::Connection,
    pub adapter: &'a str,
}

impl BluezClassic<'_> {
    fn path(&self, mac: &str) -> String {
        format!(
            "/org/bluez/{}/dev_{}",
            self.adapter,
            mac.replace(':', "_").to_uppercase()
        )
    }

    async fn bearer_connected(&self, mac: &str) -> Option<bool> {
        let reply = self
            .bus
            .call_method(
                Some("org.bluez"),
                self.path(mac).as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(BEARER_BREDR, "Connected"),
            )
            .await
            .ok()?;
        let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
        bool::try_from(value).ok()
    }

    async fn btmgmt(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new(crate::sys::tool("btmgmt"))
            .arg("--index")
            .arg(self.adapter)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| format!("btmgmt: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            Ok(text)
        } else {
            Err(format!("btmgmt: {}", text.trim()))
        }
    }
}

impl Classic for BluezClassic<'_> {
    fn disconnect(&self, mac: &str) -> impl Future<Output = Result<(), String>> + Send {
        crate::livi_sock::device_disconnect(self.bus, self.adapter, mac)
    }

    async fn bredr_up(&self, mac: &str) -> Option<bool> {
        match self.btmgmt(&["con"]).await {
            Ok(list) => Some(on_bredr(&list, mac)),
            Err(_) => self.bearer_connected(mac).await,
        }
    }

    async fn disconnect_bredr(&self, mac: &str) -> Result<&'static str, String> {
        let path = self.path(mac);
        let bearer = self.bus.call_method(
            Some("org.bluez"),
            path.as_str(),
            Some(BEARER_BREDR),
            "Disconnect",
            &(),
        );
        let bearer = match tokio::time::timeout(BEARER_WAIT, bearer).await {
            Ok(Ok(_)) => return Ok(BEARER_BREDR),
            Ok(Err(e)) => e.to_string(),
            Err(_) => format!("{BEARER_BREDR} gave no answer"),
        };
        let mac = mac.to_uppercase();
        match self.btmgmt(&["disconnect", "-t", "0", &mac]).await {
            Ok(said) if disconnected(&said) => Ok("btmgmt disconnect -t 0"),
            Ok(said) => Err(format!("{bearer}; btmgmt: {}", said.trim())),
            Err(e) => Err(format!("{bearer}; {e}")),
        }
    }
}

/// Whether `btmgmt con` lists the phone on BR/EDR (lines `78:3F:4D:62:0F:84 type BR/EDR`)
fn on_bredr(list: &str, mac: &str) -> bool {
    let mac = mac.to_uppercase();
    list.lines()
        .any(|line| line.to_uppercase().contains(&mac) && line.contains("BR/EDR"))
}

/// Whether `btmgmt disconnect` did it (`… disconnected`, not `Disconnecting … failed with status …`)
fn disconnected(said: &str) -> bool {
    said.lines()
        .any(|l| l.contains(" disconnected") && !l.contains("failed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const PHONE: &str = "78:3f:4d:62:0f:84";

    /// A Device1.Disconnect that answers (or never does), and a classic link that is up or not
    struct Fake {
        answers: Option<Result<(), String>>,
        bredr: Mutex<Option<bool>>,
        asked: Mutex<Vec<&'static str>>,
    }

    impl Fake {
        fn new(answers: Option<Result<(), String>>, bredr: Option<bool>) -> Self {
            Self {
                answers,
                bredr: Mutex::new(bredr),
                asked: Mutex::new(Vec::new()),
            }
        }
        fn asked(&self) -> Vec<&'static str> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl Classic for Fake {
        async fn disconnect(&self, _mac: &str) -> Result<(), String> {
            self.asked.lock().unwrap().push("Disconnect");
            match &self.answers {
                Some(answer) => answer.clone(),
                None => std::future::pending().await,
            }
        }
        async fn bredr_up(&self, _mac: &str) -> Option<bool> {
            *self.bredr.lock().unwrap()
        }
        async fn disconnect_bredr(&self, _mac: &str) -> Result<&'static str, String> {
            self.asked.lock().unwrap().push("bredr");
            *self.bredr.lock().unwrap() = Some(false);
            Ok("fake")
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_disconnect_that_never_answers_ends_the_classic_link_alone_once() {
        // 2026-11-19 11:26:37: Disconnect did not answer within 8 s, the iAP2 link and BR/EDR stayed until 11:34:50
        let state = HelperState::default();
        let link = state.bt_link_started(PHONE);
        let fake = Fake::new(None, Some(true));
        assert_eq!(after_disable_bluetooth(&fake, &state, PHONE).await, Ok(()));
        assert_eq!(fake.asked(), ["Disconnect", "bredr"]);
        link.close.notified().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_whole_answer_comes_before_livi_gives_up() {
        let state = HelperState::default();
        state.bt_link_started(PHONE);
        let fake = Fake::new(None, Some(true));
        let start = tokio::time::Instant::now();
        after_disable_bluetooth(&fake, &state, PHONE).await.unwrap();
        assert!(start.elapsed() < Duration::from_secs(8));
    }

    #[tokio::test(start_paused = true)]
    async fn a_phone_that_went_is_left_alone() {
        let state = HelperState::default();
        let fake = Fake::new(Some(Ok(())), Some(false));
        assert_eq!(after_disable_bluetooth(&fake, &state, PHONE).await, Ok(()));
        assert_eq!(fake.asked(), ["Disconnect"]);
        // nobody can tell, the link of the helper's is gone, Disconnect answered: done
        let fake = Fake::new(Some(Ok(())), None);
        assert_eq!(after_disable_bluetooth(&fake, &state, PHONE).await, Ok(()));
        assert_eq!(fake.asked(), ["Disconnect"]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_iap2_link_of_a_gone_br_edr_is_left_to_end_by_itself() {
        let state = HelperState::default();
        state.bt_link_started(PHONE);
        let fake = Fake::new(Some(Ok(())), Some(false));
        assert_eq!(after_disable_bluetooth(&fake, &state, PHONE).await, Ok(()));
        assert_eq!(fake.asked(), ["Disconnect"]);
        // a system that cannot tell: the helper's own link counts
        let fake = Fake::new(Some(Ok(())), None);
        assert_eq!(after_disable_bluetooth(&fake, &state, PHONE).await, Ok(()));
        assert_eq!(fake.asked(), ["Disconnect", "bredr"]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_with_the_phone_still_on_br_edr_closes_it() {
        let state = HelperState::default();
        let fake = Fake::new(Some(Ok(())), Some(true));
        assert_eq!(after_disable_bluetooth(&fake, &state, PHONE).await, Ok(()));
        assert_eq!(fake.asked(), ["Disconnect", "bredr"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_disconnect_on_a_system_that_cannot_tell_closes_br_edr() {
        let state = HelperState::default();
        let fake = Fake::new(Some(Err("Failed".into())), None);
        assert_eq!(after_disable_bluetooth(&fake, &state, PHONE).await, Ok(()));
        assert_eq!(fake.asked(), ["Disconnect", "bredr"]);
    }

    #[test]
    fn the_connection_list_of_btmgmt_names_the_bearer() {
        let list = "78:3F:4D:62:0F:84 type BR/EDR\n78:3F:4D:62:0F:84 type LE Random\n";
        assert!(on_bredr(list, PHONE));
        assert!(!on_bredr("78:3F:4D:62:0F:84 type LE Random\n", PHONE));
        assert!(!on_bredr("00:11:22:33:44:55 type BR/EDR\n", PHONE));
        assert!(!on_bredr("", PHONE));
    }

    #[test]
    fn btmgmt_says_whether_it_disconnected() {
        assert!(disconnected("78:3F:4D:62:0F:84 disconnected\n"));
        assert!(!disconnected(
            "Disconnecting 78:3F:4D:62:0F:84 failed with status 0x02 (Not Connected)\n"
        ));
        assert!(!disconnected(""));
    }
}
