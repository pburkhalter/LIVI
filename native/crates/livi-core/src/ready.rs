//! systemd's readiness protocol (sd_notify(3)): a service manager that starts LIVI as `Type=notify` gives it
//! NOTIFY_SOCKET. LIVI says `READY=1` there once it is of use: the core listens on its socket (in its folder below
//! XDG_RUNTIME_DIR), it serves org.freedesktop.Notifications, and the UI's first frame is on the display. Until then
//! `STATUS=` says what it waits for. Units ordered after LIVI start only then; one that never comes fails the start.
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

/// What LIVI tells the service manager now: READY=1 with a status once all is there, otherwise what it waits for
fn message(ui_shown: bool, notices: &Name) -> String {
    let mut waiting = Vec::new();
    if !ui_shown {
        waiting.push("the UI's first frame".to_string());
    }
    match notices {
        Name::Owned => {}
        Name::Asking => waiting.push("org.freedesktop.Notifications".into()),
        Name::Queued => {
            waiting.push("org.freedesktop.Notifications, which another server holds".into())
        }
        Name::Unserved(why) => waiting.push(format!(
            "org.freedesktop.Notifications, which LIVI cannot serve ({why})"
        )),
    }
    if waiting.is_empty() {
        "READY=1\nSTATUS=LIVI is ready: the UI on the display, the core listening, notifications served".into()
    } else {
        format!("STATUS=Waiting for {}", waiting.join(" and "))
    }
}

/// Tells the service manager how far LIVI is, until READY=1. Spawned once the core listens. `ui_shown` holds true
/// when LIVI shows no picture of its own (no compositor of LIVI's, or no UI).
pub async fn announce(mut notices: watch::Receiver<Name>, mut ui_shown: watch::Receiver<bool>) {
    let Some(socket) = socket() else { return };
    let mut told = String::new();
    let (mut notices_open, mut ui_open) = (true, true);
    loop {
        let now = message(*ui_shown.borrow_and_update(), &notices.borrow_and_update());
        if now != told {
            match send_to(socket, &now) {
                Ok(()) if now.starts_with("READY=1") => {
                    println!("[core] ready: told the service manager");
                    return;
                }
                Ok(()) => {}
                Err(e) => {
                    eprintln!("[core] the service manager cannot be told how far LIVI is: {e}");
                    return;
                }
            }
            told = now;
        }
        if !notices_open && !ui_open {
            // nothing more comes: the start times out, as it should
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
    fn ready_only_with_the_picture_and_the_notification_name() {
        assert!(message(true, &Name::Owned).starts_with("READY=1\nSTATUS="));
        assert_eq!(
            message(false, &Name::Owned),
            "STATUS=Waiting for the UI's first frame"
        );
        assert_eq!(
            message(false, &Name::Asking),
            "STATUS=Waiting for the UI's first frame and org.freedesktop.Notifications"
        );
        assert!(message(true, &Name::Queued).contains("another server holds"));
        assert!(message(true, &Name::Unserved("no session bus".into())).contains("no session bus"));
        for name in [Name::Asking, Name::Queued, Name::Unserved(String::new())] {
            assert!(!message(true, &name).contains("READY"));
        }
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
