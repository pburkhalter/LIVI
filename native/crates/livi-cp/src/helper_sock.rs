use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
pub use livi_session_io::line::Feed;
use livi_session_io::line::LineSock;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::auth_setup::MfiSigner;

pub const SOCK_PATH: &str = "/tmp/cp-bt.sock";
const TIMEOUT: Duration = Duration::from_secs(8);
const RESUBSCRIBE: Duration = Duration::from_secs(1);

/// What the helper's `start-bt` did: the link it went to, and whether that link offered the start now (false: it did once
/// more already)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BtStart {
    pub link: u64,
    pub offered: bool,
}

#[derive(Clone)]
pub struct HelperSock {
    sock: LineSock,
    protocol_major: Arc<Mutex<Option<u8>>>,
    targets: Arc<tokio::sync::Mutex<()>>,
}

impl Default for HelperSock {
    fn default() -> Self {
        Self::new(SOCK_PATH)
    }
}

fn data_of(res: &Value) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(res.get("data").and_then(Value::as_str).unwrap_or(""))
        .map_err(|e| format!("cp-bt sock bad base64: {e}"))
}

impl HelperSock {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            sock: LineSock::new(path, "cp-bt sock"),
            protocol_major: Arc::new(Mutex::new(None)),
            targets: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    async fn request(&self, line: &str) -> Result<Value, String> {
        self.sock.request(line, TIMEOUT).await
    }

    async fn request_ok(&self, line: &str) -> Result<Value, String> {
        let res = self.request(line).await?;
        if res.get("ok") == Some(&Value::Bool(true)) {
            Ok(res)
        } else {
            Err(res
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("refused")
                .to_string())
        }
    }

    pub async fn certificate(&self) -> Result<Vec<u8>, String> {
        let res = self.request_ok("certificate").await?;
        if let Some(major) = res.get("protocolMajor").and_then(Value::as_u64) {
            *self
                .protocol_major
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = u8::try_from(major).ok();
        }
        data_of(&res)
    }

    pub async fn protocol_major(&self) -> Result<u8, String> {
        let known = || {
            *self
                .protocol_major
                .lock()
                .unwrap_or_else(|e| e.into_inner())
        };
        if known().is_none() {
            self.certificate().await?;
        }
        known().ok_or_else(|| "MFi auth protocol version unknown".to_string())
    }

    pub async fn sign(&self, digest: &[u8]) -> Result<Vec<u8>, String> {
        data_of(
            &self
                .request_ok(&format!("sign {}", STANDARD.encode(digest)))
                .await?,
        )
    }

    pub async fn disconnect_bt(&self, mac: &str) -> Result<(), String> {
        self.request_ok(&format!("disconnect {mac}"))
            .await
            .map(drop)
    }

    /// The session on the phone's Bluetooth iAP2 link offers CarPlay once more (at most once per link, the helper
    /// keeps count)
    pub async fn start_bt(&self, mac: &str) -> Result<BtStart, String> {
        let res = self.request_ok(&format!("start-bt {mac}")).await?;
        Ok(BtStart {
            link: res.get("link").and_then(Value::as_u64).unwrap_or(0),
            offered: res.get("again").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// The phone's Bluetooth iAP2 link `link` (from `start_bt`) shuts down, not a later one
    pub async fn drop_bt(&self, mac: &str, link: u64) -> Result<(), String> {
        self.request_ok(&format!("drop-bt {mac} {link}"))
            .await
            .map(drop)
    }

    pub async fn drop_iap2(&self) -> Result<(), String> {
        self.request_ok("drop-iap2").await.map(drop)
    }

    pub async fn start_wired(&self, usb_udid: &str) -> Result<(), String> {
        self.request_ok(&format!("start-wired {usb_udid}"))
            .await
            .map(drop)
    }

    pub async fn reset_wired(&self, usb_udid: &str) -> Result<(), String> {
        self.request_ok(&format!("reset-wired {usb_udid}"))
            .await
            .map(drop)
    }

    pub async fn send_location(&self, nmea: &str) -> Result<(), String> {
        self.request(&format!("location {}", STANDARD.encode(nmea)))
            .await
            .map(drop)
    }

    pub async fn send_vehicle_status(&self, status: &Value) -> Result<(), String> {
        self.request(&format!("vehicle-status {status}"))
            .await
            .map(drop)
    }

    pub async fn seek(&self, ms: u32, bt_mac: Option<&str>) -> Result<(), String> {
        let line = match bt_mac {
            Some(mac) => format!("seek {ms} {mac}"),
            None => format!("seek {ms}"),
        };
        self.request(&line).await.map(drop)
    }

    /// An order for a call (livi_runtime::calls::Order, e.g. `dial +41790000001`) to the session of the phone with
    /// this Bluetooth address, or to any
    pub async fn call(&self, order: &str, bt_mac: Option<&str>) -> Result<(), String> {
        self.request_ok(&format!("call {} {order}", bt_mac.unwrap_or("-")))
            .await
            .map(drop)
    }

    pub async fn send_reconnect_targets(
        &self,
        targets: &[(String, Option<String>)],
    ) -> Result<(), String> {
        let _in_turn = self.targets.lock().await;
        let list: Vec<Value> = targets
            .iter()
            .map(|(mac, uuid)| Value::Array(vec![mac.clone().into(), uuid.clone().into()]))
            .collect();
        self.request(&format!("reconnect-targets {}", Value::Array(list)))
            .await
            .map(drop)
    }

    pub async fn open_tunnel(&self, cid: &str, bt_mac: &str) -> std::io::Result<UnixStream> {
        let mut sock = UnixStream::connect(self.sock.path()).await?;
        let header = format!("tunnel {cid} {bt_mac}");
        sock.write_all(format!("{}\n", header.trim_end()).as_bytes())
            .await?;
        Ok(sock)
    }

    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Feed> {
        self.sock.subscribe(RESUBSCRIBE, |_| true)
    }
}

impl MfiSigner for HelperSock {
    fn certificate(&self) -> impl Future<Output = Result<Vec<u8>, String>> + Send {
        HelperSock::certificate(self)
    }

    fn sign(&self, digest: &[u8]) -> impl Future<Output = Result<Vec<u8>, String>> + Send {
        let digest = digest.to_vec();
        async move { HelperSock::sign(self, &digest).await }
    }

    fn protocol_major(&self) -> impl Future<Output = Result<u8, String>> + Send {
        HelperSock::protocol_major(self)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::UnixListener;

    use super::*;

    pub(crate) struct Dir(pub PathBuf);

    impl Dir {
        pub(crate) fn new() -> Self {
            let n: u64 = rand_suffix();
            let dir = std::env::temp_dir().join(format!("lcp-{n:x}"));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn rand_suffix() -> u64 {
        u64::from_le_bytes(crate::crypto::random_bytes::<8>())
    }

    pub(crate) fn serve(
        path: &Path,
        reply: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Arc<Mutex<Vec<String>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let listener = UnixListener::bind(path).unwrap();
        let reply = Arc::new(reply);
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let reply = reply.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let (rd, mut wr) = sock.into_split();
                    let mut lines = BufReader::new(rd).lines();
                    if let Ok(Some(line)) = lines.next_line().await {
                        log.lock().unwrap().push(line.clone());
                        if let Some(answer) = reply(&line) {
                            let _ = wr.write_all(answer.as_bytes()).await;
                        }
                    }
                });
            }
        });
        seen
    }

    #[tokio::test]
    async fn the_certificate_brings_the_protocol_generation() {
        let dir = Dir::new();
        let path = dir.0.join("cp.sock");
        let seen = serve(&path, |line| match line {
            "certificate" => {
                Some(r#"{"ok":true,"data":"AQID","protocolMajor":3}"#.to_string() + "\n")
            }
            l if l.starts_with("sign ") => Some(r#"{"ok":true,"data":"BAU="}"#.to_string() + "\n"),
            _ => None,
        });
        let helper = HelperSock::new(&path);
        assert_eq!(helper.protocol_major().await, Ok(3));
        assert_eq!(helper.certificate().await, Ok(vec![1, 2, 3]));
        assert_eq!(helper.sign(&[9]).await, Ok(vec![4, 5]));
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["certificate", "certificate", "sign CQ=="]
        );
    }

    #[tokio::test]
    async fn a_refusal_carries_the_helpers_reason() {
        let dir = Dir::new();
        let path = dir.0.join("cp.sock");
        serve(&path, |line| match line {
            "drop-iap2" => Some("{\"ok\":false,\"error\":\"busy\"}\n".into()),
            "certificate" => Some("{\"ok\":true,\"data\":\"\"}\n".into()),
            "location TU9W" => Some("{\"ok\":false,\"error\":\"nobody\"}\n".into()),
            "disconnect AA:BB" => Some("not json\n".into()),
            _ => Some("{\"ok\":true}".into()),
        });
        let helper = HelperSock::new(&path);
        assert_eq!(helper.drop_iap2().await, Err("busy".into()));
        assert_eq!(
            helper.protocol_major().await,
            Err("MFi auth protocol version unknown".into())
        );
        assert_eq!(helper.send_location("MOV").await, Ok(()));
        assert!(
            helper
                .disconnect_bt("AA:BB")
                .await
                .unwrap_err()
                .contains("bad json")
        );
        let missing = HelperSock::new(dir.0.join("nothing.sock"));
        assert!(
            missing
                .start_wired("u")
                .await
                .unwrap_err()
                .contains("cp-bt sock error")
        );
    }

    #[tokio::test]
    async fn plain_commands_go_out_as_the_helper_expects_them() {
        let dir = Dir::new();
        let path = dir.0.join("cp.sock");
        let seen = serve(&path, |_| Some("{\"ok\":true}\n".into()));
        let helper = HelperSock::new(&path);
        helper.disconnect_bt("AA:BB").await.unwrap();
        helper.start_wired("udid1").await.unwrap();
        helper
            .send_vehicle_status(&serde_json::json!({ "range": 120 }))
            .await
            .unwrap();
        helper
            .send_reconnect_targets(&[("AA".into(), Some("u".into())), ("BB".into(), None)])
            .await
            .unwrap();
        helper.seek(61000, Some("AA:BB")).await.unwrap();
        helper.seek(0, None).await.unwrap();
        helper.drop_bt("AA:BB", 3).await.unwrap();
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [
                "disconnect AA:BB",
                "start-wired udid1",
                "vehicle-status {\"range\":120}",
                "reconnect-targets [[\"AA\",\"u\"],[\"BB\",null]]",
                "seek 61000 AA:BB",
                "seek 0",
                "drop-bt AA:BB 3",
            ]
        );
    }

    #[tokio::test]
    async fn a_start_over_bluetooth_names_its_link() {
        let dir = Dir::new();
        let path = dir.0.join("cp.sock");
        serve(&path, |line| match line {
            "start-bt AA:BB" => Some("{\"ok\":true,\"link\":4,\"again\":true}\n".into()),
            "start-bt CC:DD" => Some("{\"ok\":true,\"link\":5,\"again\":false}\n".into()),
            _ => Some("{\"ok\":false,\"error\":\"no link\"}\n".into()),
        });
        let helper = HelperSock::new(&path);
        assert_eq!(
            helper.start_bt("AA:BB").await,
            Ok(BtStart {
                link: 4,
                offered: true
            })
        );
        assert_eq!(
            helper.start_bt("CC:DD").await,
            Ok(BtStart {
                link: 5,
                offered: false
            })
        );
        assert_eq!(helper.start_bt("EE:FF").await, Err("no link".into()));
    }

    #[tokio::test]
    async fn the_tunnel_names_the_session() {
        let dir = Dir::new();
        let path = dir.0.join("cp.sock");
        let seen = serve(&path, |_| None);
        let helper = HelperSock::new(&path);
        drop(helper.open_tunnel("cid1", "").await.unwrap());
        drop(helper.open_tunnel("cid2", "AA:BB").await.unwrap());
        for _ in 0..100 {
            if seen.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut lines = seen.lock().unwrap().clone();
        lines.sort();
        assert_eq!(lines, ["tunnel cid1", "tunnel cid2 AA:BB"]);
    }

    #[tokio::test]
    async fn the_subscription_comes_back_after_a_loss() {
        let dir = Dir::new();
        let path = dir.0.join("cp.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let helper = HelperSock::new(&path);
        let mut feed = helper.subscribe();

        let (sock, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = sock.into_split();
        let mut lines = BufReader::new(rd).lines();
        assert_eq!(
            lines.next_line().await.unwrap().as_deref(),
            Some("subscribe")
        );
        assert_eq!(feed.recv().await, Some(Feed::Connected));
        wr.write_all(b"{\"type\":\"wifi\"}\n\nnot json\n[1]\n")
            .await
            .unwrap();
        assert_eq!(
            feed.recv().await,
            Some(Feed::Event(serde_json::json!({ "type": "wifi" })))
        );
        drop((lines, wr));

        let (again, _) = listener.accept().await.unwrap();
        assert_eq!(feed.recv().await, Some(Feed::Connected));
        drop(feed);
        drop(again);
    }
}
