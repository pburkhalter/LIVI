//! systemd's readiness protocol (sd_notify(3)): a service manager that starts LIVI as `Type=notify` gives it
//! NOTIFY_SOCKET. LIVI says `READY=1` there once it is of use: the UI's first frame is on the display, and the core
//! listens on its socket (in its folder below XDG_RUNTIME_DIR; it does before it starts the compositor and the UI).
//! Until then `STATUS=` says what it waits for. Units ordered after LIVI start only then; a READY=1 that never comes
//! fails the start. The notifications do not hold LIVI back: whether org.freedesktop.Notifications is LIVI's yet, or
//! another server holds it, or there is no session bus, `STATUS=` says, before READY=1 and after it as it changes.
//! Without NOTIFY_SOCKET nothing of this happens.

use std::ffi::{OsStr, OsString};
use std::sync::OnceLock;

use tokio::sync::watch;

use crate::notices::Name;

const VARIABLE: &str = "NOTIFY_SOCKET";

static SOCKET: OnceLock<Option<OsString>> = OnceLock::new();

/// Takes NOTIFY_SOCKET out of the environment, as sd_notify does with unset_environment: the core speaks to the service
/// manager, the programs it starts do not. Call it before there is a second thread.
pub fn take_socket() {
    let socket = std::env::var_os(VARIABLE);
    if socket.is_some() {
        // SAFETY: main calls it before the runtime starts: no other thread reads or writes the environment
        unsafe { std::env::remove_var(VARIABLE) };
    }
    let _ = SOCKET.set(socket);
}

fn socket() -> Option<&'static OsStr> {
    SOCKET.get().and_then(|s| s.as_deref())
}

/// For a core that replaces itself (exec): the variable again, so that the new one can say READY=1 as well
pub fn for_exec() -> Option<(&'static str, &'static OsStr)> {
    socket().map(|s| (VARIABLE, s))
}

/// One datagram to the service manager's socket: a path, or an abstract name after `@` (sd_notify(3))
#[cfg(target_os = "linux")]
fn send_to(socket: &OsStr, state: &str) -> std::io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    let out = UnixDatagram::unbound()?;
    let address = match socket.as_bytes().strip_prefix(b"@") {
        Some(name) => SocketAddr::from_abstract_name(name)?,
        None => SocketAddr::from_pathname(std::path::Path::new(socket))?,
    };
    out.send_to_addr(state.as_bytes(), &address)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn send_to(_socket: &OsStr, _state: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("sd_notify is Linux's"))
}

/// What the status says of the notification name
fn notifications(notices: &Name) -> String {
    match notices {
        Name::Owned => "notifications served".into(),
        Name::Asking => "notifications: asking for org.freedesktop.Notifications".into(),
        Name::Queued => {
            "notifications wait for org.freedesktop.Notifications, which another server holds"
                .into()
        }
        Name::Unserved(why) => format!("no notifications ({why})"),
    }
}

/// What LIVI tells the service manager now: READY=1 once the UI's first frame is on the display, the status always
fn message(ui_shown: bool, notices: &Name) -> String {
    if ui_shown {
        format!(
            "READY=1\nSTATUS=LIVI is ready: the UI on the display, the core listening; {}",
            notifications(notices)
        )
    } else {
        format!(
            "STATUS=Waiting for the UI's first frame; {}",
            notifications(notices)
        )
    }
}

/// Whether nothing more of the notification name can change
fn settled(notices: &Name) -> bool {
    matches!(notices, Name::Owned | Name::Unserved(_))
}

/// Tells the service manager how far LIVI is: READY=1 with the UI's first frame, the status until the notification
/// name is settled. Spawned once the core listens. `ui_shown` holds true when LIVI shows no picture of its own (no
/// compositor of LIVI's, or no UI).
pub async fn announce(mut notices: watch::Receiver<Name>, mut ui_shown: watch::Receiver<bool>) {
    let Some(socket) = socket() else { return };
    let mut told = String::new();
    let mut ready = false;
    let (mut notices_open, mut ui_open) = (true, true);
    loop {
        let shown = *ui_shown.borrow_and_update();
        let name = notices.borrow_and_update().clone();
        let mut now = message(shown, &name);
        if ready {
            // READY=1 once, then the status alone
            now = now.replacen("READY=1\n", "", 1);
        }
        if now != told {
            if let Err(e) = send_to(socket, &now) {
                eprintln!("[core] the service manager cannot be told how far LIVI is: {e}");
                return;
            }
            if now.starts_with("READY=1") {
                println!(
                    "[core] ready: told the service manager ({})",
                    notifications(&name)
                );
                ready = true;
            }
            told = now;
        }
        if ready && settled(&name) {
            return;
        }
        if !notices_open && !ui_open {
            // nothing more comes; without READY=1 the start times out, as it should
            return;
        }
        tokio::select! {
            r = notices.changed(), if notices_open => notices_open = r.is_ok(),
            r = ui_shown.changed(), if ui_open => ui_open = r.is_ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_with_the_picture_whatever_the_notifications_do() {
        assert_eq!(
            message(true, &Name::Owned),
            "READY=1\nSTATUS=LIVI is ready: the UI on the display, the core listening; notifications served"
        );
        // CarPlay never waits for the notification server: READY=1 all the same, the status says why
        for name in [
            Name::Asking,
            Name::Queued,
            Name::Unserved("no session bus".into()),
        ] {
            assert!(message(true, &name).starts_with("READY=1\n"), "{name:?}");
        }
        assert!(message(true, &Name::Queued).ends_with("which another server holds"));
        assert!(message(true, &Name::Unserved("no session bus".into())).contains("no session bus"));
        // only the picture holds it back
        for name in [Name::Owned, Name::Asking, Name::Queued] {
            let m = message(false, &name);
            assert!(!m.contains("READY"), "{m}");
            assert!(
                m.starts_with("STATUS=Waiting for the UI's first frame; "),
                "{m}"
            );
        }
        assert!(settled(&Name::Owned) && settled(&Name::Unserved(String::new())));
        assert!(!settled(&Name::Asking) && !settled(&Name::Queued));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_service_manager_gets_one_datagram_at_a_path_or_an_abstract_name() {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::{SocketAddr, UnixDatagram};

        let dir = crate::config_file::tests::TempDir::new();
        let path = dir.0.join("notify");
        let manager = UnixDatagram::bind(&path).unwrap();
        send_to(path.as_os_str(), "READY=1\nSTATUS=x").unwrap();
        let mut buf = [0u8; 64];
        let n = manager.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1\nSTATUS=x");

        let name = format!("livi-ready-test-{}", std::process::id());
        let manager =
            UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap())
                .unwrap();
        send_to(OsStr::new(&format!("@{name}")), "STATUS=y").unwrap();
        let n = manager.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"STATUS=y");
    }
}
