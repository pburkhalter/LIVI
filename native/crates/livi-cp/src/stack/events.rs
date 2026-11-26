use super::*;

impl<M: Media> Conn<M> {
    pub(super) fn on_event_connection(&mut self, sock: TcpStream) {
        println!("[cpStack] event channel connected");
        let Some(shared) = self.pair_verify.shared_secret() else {
            return;
        };
        let write = hkdf_sha512(&shared, b"Events-Salt", b"Events-Write-Encryption-Key");
        let read = hkdf_sha512(&shared, b"Events-Salt", b"Events-Read-Encryption-Key");
        let (rd, wr) = sock.into_split();
        self.report_unanswered("when a new channel replaces it");
        self.event = Some(EventConn {
            read: rd,
            write: wr,
            cipher: ControlCipher::new(read, write),
            sealed: Vec::new(),
            plain: Vec::new(),
            cseq: 0,
            sent: std::collections::VecDeque::new(),
            notes: 0,
        });
    }

    /// Names the commands on the event channel that never got an answer, when the channel goes or the session ends
    pub(super) fn report_unanswered(&self, when: &str) {
        let Some(ev) = self.event.as_ref() else {
            return;
        };
        if !ev.sent.is_empty() {
            println!(
                "[cpStack] event unanswered {when}: {} of {} sent, {}",
                ev.sent.len(),
                ev.cseq,
                pending(&ev.sent, Instant::now())
            );
        }
    }

    pub(super) async fn on_event_bytes(&mut self, chunk: &[u8]) {
        let debug = self.ctx.debug;
        let Some(ev) = self.event.as_mut() else {
            return;
        };
        ev.sealed.extend_from_slice(chunk);
        match ev.cipher.decrypt(&ev.sealed) {
            Ok((data, rest)) => {
                ev.sealed = rest;
                ev.plain.extend(data);
            }
            Err(_) => {
                println!("[cpStack] event channel decrypt failed");
                return;
            }
        }
        let (messages, rest) = rtsp::parse(&ev.plain);
        ev.plain = rest;
        for msg in messages {
            if msg.method.starts_with("RTSP/") || msg.method.starts_with("HTTP/") {
                let cseq = msg.headers.get("cseq").and_then(|c| c.parse::<u32>().ok());
                let now = Instant::now();
                let command = answered(&mut ev.sent, cseq);
                let took = command
                    .as_ref()
                    .map(|c| now.saturating_duration_since(c.at));
                if cseq.is_some() && cseq == self.view_area_cseq {
                    self.view_area_cseq = None;
                    if msg.path != "200" {
                        println!(
                            "[cpStack] the phone refused the view area ({}): it connects anew",
                            msg.path
                        );
                        self.view_area_refused = true;
                    }
                }
                let shown = !msg.path.starts_with('2') || (debug && !msg.body.is_empty());
                let late = took.is_some_and(|t| t >= SLOW_ANSWER);
                if shown || (late && note(&mut ev.notes, CHANNEL_NOTES)) {
                    let answer = Answer {
                        cseq,
                        command: command.as_ref().zip(took),
                        last_sent: ev.cseq,
                        late_only: !shown,
                    };
                    println!("{}", answer_line(&msg, &answer, &ev.sent, now));
                }
                continue;
            }
            let decoded = if msg.body.is_empty() {
                String::new()
            } else {
                match bplist::decode(&msg.body) {
                    Ok(v) => format!(" {v:?}"),
                    Err(_) => format!(" ({}B non-plist body)", msg.body.len()),
                }
            };
            println!("[cpStack] event < {} {}{decoded}", msg.method, msg.path);
            let answer = rtsp::build_response(&msg, status(200));
            let sealed = ev.cipher.encrypt(&answer);
            if ev.write.write_all(&sealed).await.is_err() {
                return;
            }
        }
    }

    pub(super) async fn send_event_command(&mut self, body: &Value) {
        self.send_event_command_with(body, "").await;
    }

    pub(super) async fn send_event_command_with(&mut self, body: &Value, headers: &str) {
        let debug = self.ctx.debug;
        let kind = body
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("untyped");
        let Some(ev) = self.event.as_mut() else {
            if note(&mut self.event_dropped, DROPPED_NOTES) {
                println!("[cpStack] event dropped: {kind}, no event channel");
            }
            return;
        };
        ev.cseq += 1;
        let now = Instant::now();
        if let Some(lost) = remember(&mut ev.sent, ev.cseq, kind, now)
            && note(&mut ev.notes, CHANNEL_NOTES)
        {
            println!(
                "[cpStack] event lost: no answer to {} (CSeq {}) in {} ms, it is no longer kept",
                lost.kind,
                lost.cseq,
                now.saturating_duration_since(lost.at).as_millis()
            );
        }
        if debug && !matches!(kind, "hidSendReport" | "iAPSendMessage") {
            println!(
                "[cpStack] event > {}{body:?} (CSeq {})",
                headers.replace("\r\n", " "),
                ev.cseq
            );
        }
        let payload = bplist::encode(body);
        let mut msg = format!(
            "POST /command RTSP/1.0\r\nContent-Type: {PLIST}\r\nContent-Length: {}\r\nCSeq: {}\r\n{headers}\r\n",
            payload.len(),
            ev.cseq
        )
        .into_bytes();
        msg.extend_from_slice(&payload);
        let sealed = ev.cipher.encrypt(&msg);
        if let Err(e) = ev.write.write_all(&sealed).await
            && note(&mut ev.notes, CHANNEL_NOTES)
        {
            println!("[cpStack] event not sent: {kind} (CSeq {}): {e}", ev.cseq);
        }
    }

    pub(super) async fn send_hid(&mut self, uid: u32, report: &[u8]) {
        self.send_event_command(&dict([
            ("type", Value::String("hidSendReport".into())),
            ("uuid", Value::String(format!("{uid:x}"))),
            ("hidReport", Value::Data(report.to_vec())),
        ]))
        .await;
    }

    pub(super) async fn send_night_mode(&mut self, night: bool) {
        self.send_event_command(&dict([
            ("type", Value::String("setNightMode".into())),
            ("params", dict([("nightMode", Value::Bool(night))])),
        ]))
        .await;
        self.send_appearance(self.main_uuid(), night).await;
    }

    pub(super) async fn send_cluster_appearance(&mut self, night: bool) {
        if self.ctx.config.borrow().info.cluster.is_none() {
            return;
        }
        self.send_appearance(self.alt_uuid(), night).await;
    }

    pub(super) async fn send_appearance(&mut self, uuid: String, night: bool) {
        for kind in ["uiAppearanceUpdate", "mapAppearanceUpdate"] {
            self.send_event_command(&dict([
                ("type", Value::String(kind.into())),
                (
                    "params",
                    dict([
                        ("uuid", Value::String(uuid.clone())),
                        ("appearanceMode", Value::Int(u64::from(night))),
                        ("appearanceSetting", Value::Int(APPEARANCE_AUTOMATIC)),
                    ]),
                ),
            ]))
            .await;
        }
    }

    pub(super) async fn send_limited_ui(&mut self, limited: bool) {
        self.send_event_command(&dict([
            ("type", Value::String("setLimitedUI".into())),
            ("params", dict([("limitedUI", Value::Bool(limited))])),
        ]))
        .await;
    }

    pub(super) async fn force_keyframe(&mut self, uuid: &str) {
        self.send_event_command(&dict([
            ("type", Value::String("forceKeyFrame".into())),
            ("params", dict([("uuid", Value::String(uuid.into()))])),
        ]))
        .await;
    }

    pub(super) async fn activate_cluster_stream(&mut self) {
        let url = self.wanted_cluster_url();
        self.send_event_command(&dict([
            ("type", Value::String("showUI".into())),
            (
                "params",
                dict([
                    ("uuid", Value::String(self.alt_uuid())),
                    ("url", Value::String(url.clone())),
                ]),
            ),
        ]))
        .await;
        self.cluster_url = Some(url);
        self.force_keyframe(&self.alt_uuid()).await;
    }

    pub(super) fn main_uuid(&self) -> String {
        info::display_uuids(&self.ctx.config.borrow().info).0
    }

    pub(super) fn alt_uuid(&self) -> String {
        info::display_uuids(&self.ctx.config.borrow().info).1
    }

    pub(super) fn wanted_cluster_url(&self) -> String {
        let url = &self.ctx.config.borrow().cluster_url;
        if url.is_empty() {
            CLUSTER_MAP_URL.into()
        } else {
            url.clone()
        }
    }

    /// Moves the phone's picture into the view area the config asks for now, if the phone knows it: the areas it got
    /// in /info are still the same (otherwise only a new connection tells it)
    pub(super) async fn follow_view_area(&mut self) {
        let (areas, index) = {
            let cfg = self.ctx.config.borrow();
            (
                cfg.info.main.view_areas.clone(),
                cfg.info.main.view_area_index,
            )
        };
        let Some((told, told_index)) = &self.view_areas_told else {
            return;
        };
        if *told != areas || *told_index == index || index >= areas.len() || self.event.is_none() {
            return;
        }
        println!("[cpStack] view area {index} of {}", areas.len());
        self.view_areas_told = Some((areas.clone(), index));
        let command = info::view_area_command(&self.main_uuid(), index, areas.len());
        self.send_event_command(&command).await;
        self.view_area_cseq = self.event.as_ref().map(|ev| ev.cseq);
    }

    pub(super) async fn follow_cluster_url(&mut self) {
        if self.cluster_url.is_none()
            || self.cluster_url == Some(self.wanted_cluster_url())
            || !self.cluster_want
            || self.event.is_none()
            || !self.main_stream_ready
        {
            return;
        }
        self.activate_cluster_stream().await;
    }
}

/// How many unanswered commands are kept: the phone answers each, a lost answer drops out at the front
const SENT_KEPT: usize = 64;
/// An answer that takes this long is logged beside its command
const SLOW_ANSWER: Duration = Duration::from_secs(1);
/// How many lines one event channel prints at most about late, lost and unsent commands
const CHANNEL_NOTES: (u32, &str) = (16, "late, lost and unsent commands of this event channel");
/// How many lines one session prints at most about commands dropped for want of an event channel
const DROPPED_NOTES: (u32, &str) = (16, "commands dropped for want of an event channel");
/// How many runs of waiting commands a line names, the oldest first
const RUNS_NAMED: usize = 8;
/// How many headers of a refusal a line names, and how much of a value, a status line or a body it shows
const HEADERS_NAMED: usize = 12;
const VALUE_KEPT: usize = 64;
const STATUS_KEPT: usize = 80;
const BODY_KEPT: usize = 200;
/// The headers whose value a refusal shows; of the others only the name
const HEADER_VALUES: [&str; 6] = [
    "cseq",
    "content-length",
    "content-type",
    "server",
    "connection",
    "retry-after",
];

/// A command on the event channel that has no answer yet
pub(super) struct Sent {
    cseq: u32,
    kind: String,
    at: Instant,
}

/// What an answer on the event channel belongs to
struct Answer<'a> {
    cseq: Option<u32>,
    /// The waiting command it answers, and how long the answer took
    command: Option<(&'a Sent, Duration)>,
    /// The CSeq of the last command sent on the channel
    last_sent: u32,
    /// Logged only because it came late: an answer the phone gave, without its body
    late_only: bool,
}

/// Whether one more line of a kind may go out (`limit` of them); the first one over the limit says that the rest
/// goes unlogged
fn note(count: &mut u32, (limit, what): (u32, &str)) -> bool {
    *count = count.saturating_add(1);
    if *count == limit + 1 {
        println!("[cpStack] event notes: further {what} go unlogged");
    }
    *count <= limit
}

/// Keeps a command until its answer comes; hands back the oldest one when the list is full
fn remember(
    sent: &mut std::collections::VecDeque<Sent>,
    cseq: u32,
    kind: &str,
    at: Instant,
) -> Option<Sent> {
    let lost = if sent.len() == SENT_KEPT {
        sent.pop_front()
    } else {
        None
    };
    sent.push_back(Sent {
        cseq,
        kind: kind.to_string(),
        at,
    });
    lost
}

/// The command an answer belongs to, taken off the list
fn answered(sent: &mut std::collections::VecDeque<Sent>, cseq: Option<u32>) -> Option<Sent> {
    let cseq = cseq?;
    let at = sent.iter().position(|s| s.cseq == cseq)?;
    sent.remove(at)
}

/// The log line of an answer: its status line and the command it answers; a refusal also with its headers, the
/// commands still waiting if it answers none of them, and at most the start of its body; a late one alone as such
fn answer_line(
    msg: &rtsp::Request,
    answer: &Answer,
    sent: &std::collections::VecDeque<Sent>,
    now: Instant,
) -> String {
    let what = match (answer.command, answer.cseq) {
        (Some((c, took)), _) => format!(
            " to {} (CSeq {}) after {} ms",
            c.kind,
            c.cseq,
            took.as_millis()
        ),
        (None, Some(n)) => format!(
            ", CSeq {n} has no waiting command (last sent {})",
            answer.last_sent
        ),
        (None, None) => ", no CSeq".to_string(),
    };
    let head = if answer.late_only { "late" } else { "response" };
    let mut line = format!("[cpStack] event {head} {}{what}", status_line(msg));
    if answer.late_only {
        return line;
    }
    if msg.path.starts_with('2') {
        if let Some(body) = body_named(&msg.body, usize::MAX) {
            line.push_str(&format!(" {body}"));
        }
        return line;
    }
    line.push_str(&format!("; {}", headers_named(&msg.headers)));
    if answer.command.is_none() {
        line.push_str(&format!("; waiting: {}", pending(sent, now)));
    }
    if let Some(body) = body_named(&msg.body, BODY_KEPT) {
        line.push_str(&format!("; body {body}"));
    }
    line
}

/// An answer's status line code first, as the log showed it before: "408 Request Timeout (RTSP/1.0)"
fn status_line(msg: &rtsp::Request) -> String {
    let status = msg
        .line
        .strip_prefix(msg.method.as_str())
        .unwrap_or(&msg.line);
    format!(
        "{} ({})",
        cut(status.trim(), STATUS_KEPT),
        cut(&msg.method, VALUE_KEPT)
    )
}

/// A refusal's headers by name, sorted; with the value only for those that say how it was answered
fn headers_named(headers: &std::collections::HashMap<String, String>) -> String {
    let mut names: Vec<&String> = headers.keys().collect();
    names.sort();
    let shown: Vec<String> = names
        .iter()
        .take(HEADERS_NAMED)
        .map(|name| {
            if HEADER_VALUES.contains(&name.as_str()) {
                format!("{name}: {}", cut(&headers[*name], VALUE_KEPT))
            } else {
                cut(name, VALUE_KEPT)
            }
        })
        .collect();
    match names.len().saturating_sub(HEADERS_NAMED) {
        _ if shown.is_empty() => "no headers".to_string(),
        0 => format!("headers {}", shown.join(", ")),
        more => format!("headers {} and {more} more", shown.join(", ")),
    }
}

/// The commands still waiting for an answer, oldest first, a run of one kind together:
/// "setLimitedUI (CSeq 6, 5206 ms), iAPSendMessage x3 (CSeq 7-9, 5100-4800 ms)"
fn pending(sent: &std::collections::VecDeque<Sent>, now: Instant) -> String {
    let mut runs: Vec<(&Sent, &Sent, usize)> = Vec::new();
    for s in sent {
        match runs.last_mut() {
            Some((first, last, n)) if first.kind == s.kind => {
                *last = s;
                *n += 1;
            }
            _ => runs.push((s, s, 1)),
        }
    }
    let ms = |s: &Sent| now.saturating_duration_since(s.at).as_millis();
    let mut named: Vec<String> = runs
        .iter()
        .take(RUNS_NAMED)
        .map(|(first, last, n)| match n {
            1 => format!("{} (CSeq {}, {} ms)", first.kind, first.cseq, ms(first)),
            _ => format!(
                "{} x{n} (CSeq {}-{}, {}-{} ms)",
                first.kind,
                first.cseq,
                last.cseq,
                ms(first),
                ms(last)
            ),
        })
        .collect();
    let rest: usize = runs.iter().skip(RUNS_NAMED).map(|(_, _, n)| n).sum();
    if rest > 0 {
        named.push(format!("and {rest} more"));
    }
    if named.is_empty() {
        return "none".to_string();
    }
    named.join(", ")
}

/// An answer's body as the log shows it: a plist decoded and cut to `kept` characters, otherwise its size
fn body_named(body: &[u8], kept: usize) -> Option<String> {
    match bplist::decode(body) {
        Ok(v) => Some(cut(&format!("{v:?}"), kept)),
        Err(_) if body.is_empty() => None,
        Err(_) => Some(format!("({}B non-plist body)", body.len())),
    }
}

/// At most `kept` characters of what the phone sent, control characters as "?"
fn cut(text: &str, kept: usize) -> String {
    let mut out: String = text
        .chars()
        .take(kept)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if text.chars().nth(kept).is_some() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod sent_tests {
    use std::collections::{HashMap, VecDeque};
    use std::time::{Duration, Instant};

    use super::{
        Answer, CHANNEL_NOTES, SENT_KEPT, answer_line, answered, cut, headers_named, note, pending,
        remember,
    };
    use crate::rtsp;

    #[test]
    fn an_answer_names_its_command() {
        let at = Instant::now();
        let mut sent = VecDeque::new();
        remember(&mut sent, 1, "setNightMode", at);
        remember(&mut sent, 2, "setLimitedUI", at);
        assert_eq!(
            answered(&mut sent, Some(2)).map(|s| s.kind).as_deref(),
            Some("setLimitedUI")
        );
        assert!(answered(&mut sent, Some(2)).is_none());
        assert!(answered(&mut sent, None).is_none());
        assert_eq!(
            answered(&mut sent, Some(1)).map(|s| s.kind).as_deref(),
            Some("setNightMode")
        );
        assert!(sent.is_empty());
    }

    #[test]
    fn unanswered_ones_drop_out_and_are_handed_back() {
        let at = Instant::now();
        let mut sent = VecDeque::new();
        let mut lost = Vec::new();
        for n in 1..=(SENT_KEPT as u32 + 3) {
            lost.extend(remember(&mut sent, n, "hidSendReport", at).map(|s| s.cseq));
        }
        assert_eq!(sent.len(), SENT_KEPT);
        assert_eq!(lost, [1, 2, 3]);
        assert!(answered(&mut sent, Some(1)).is_none());
        assert!(answered(&mut sent, Some(SENT_KEPT as u32 + 3)).is_some());
    }

    #[test]
    fn waiting_commands_go_together_by_kind_oldest_first() {
        let at = Instant::now();
        let mut sent = VecDeque::new();
        assert_eq!(pending(&sent, at), "none");
        remember(&mut sent, 6, "setLimitedUI", at);
        for n in 7..=9 {
            remember(
                &mut sent,
                n,
                "iAPSendMessage",
                at + Duration::from_millis(100 * u64::from(n)),
            );
        }
        assert_eq!(
            pending(&sent, at + Duration::from_millis(5206)),
            "setLimitedUI (CSeq 6, 5206 ms), iAPSendMessage x3 (CSeq 7-9, 4506-4306 ms)"
        );
        let mut many = VecDeque::new();
        for n in 1..=12 {
            remember(&mut many, n, if n % 2 == 0 { "a" } else { "b" }, at);
        }
        assert!(pending(&many, at).ends_with("a (CSeq 8, 0 ms), and 4 more"));
    }

    fn answer_of(raw: &[u8]) -> rtsp::Request {
        rtsp::parse(raw).0.remove(0)
    }

    #[test]
    fn a_refusal_without_cseq_shows_its_status_line_headers_and_what_waits() {
        let at = Instant::now();
        let mut sent = VecDeque::new();
        remember(&mut sent, 5, "mapAppearanceUpdate", at);
        remember(&mut sent, 6, "setLimitedUI", at);
        answered(&mut sent, Some(5));
        let msg =
            answer_of(b"RTSP/1.0 408 Request Timeout\r\nContent-Length: 0\r\nX-Secret: k\r\n\r\n");
        let answer = Answer {
            cseq: None,
            command: None,
            last_sent: 6,
            late_only: false,
        };
        assert_eq!(
            answer_line(&msg, &answer, &sent, at + Duration::from_millis(5206)),
            "[cpStack] event response 408 Request Timeout (RTSP/1.0), no CSeq; \
             headers content-length: 0, x-secret; waiting: setLimitedUI (CSeq 6, 5206 ms)"
        );
    }

    #[test]
    fn a_refusal_with_its_cseq_names_the_command_and_how_long_it_took() {
        let at = Instant::now();
        let mut sent = VecDeque::new();
        remember(&mut sent, 6, "setLimitedUI", at);
        let msg = answer_of(b"HTTP/1.1 408 Request Timeout\r\nCSeq: 6\r\n\r\n");
        let command = answered(&mut sent, Some(6)).unwrap();
        let answer = Answer {
            cseq: Some(6),
            command: Some((&command, Duration::from_millis(5206))),
            last_sent: 6,
            late_only: false,
        };
        assert_eq!(
            answer_line(&msg, &answer, &sent, at),
            "[cpStack] event response 408 Request Timeout (HTTP/1.1) to setLimitedUI (CSeq 6) \
             after 5206 ms; headers cseq: 6"
        );
        let stray = Answer {
            cseq: Some(4),
            command: None,
            last_sent: 6,
            late_only: false,
        };
        let msg = answer_of(b"RTSP/1.0 400 Bad Request\r\nCSeq: 4\r\n\r\n");
        assert!(
            answer_line(&msg, &stray, &sent, at).contains(
                "400 Bad Request (RTSP/1.0), CSeq 4 has no waiting command (last sent 6)"
            )
        );
    }

    #[test]
    fn a_late_answer_shows_no_headers_and_no_body() {
        let at = Instant::now();
        let mut sent = VecDeque::new();
        remember(&mut sent, 3, "forceKeyFrame", at);
        let command = answered(&mut sent, Some(3)).unwrap();
        let msg = answer_of(b"RTSP/1.0 200 OK\r\nCSeq: 3\r\nContent-Length: 2\r\n\r\nxy");
        let answer = Answer {
            cseq: Some(3),
            command: Some((&command, Duration::from_millis(1500))),
            last_sent: 3,
            late_only: true,
        };
        assert_eq!(
            answer_line(&msg, &answer, &sent, at),
            "[cpStack] event late 200 OK (RTSP/1.0) to forceKeyFrame (CSeq 3) after 1500 ms"
        );
    }

    #[test]
    fn what_the_phone_sends_is_bounded() {
        assert_eq!(cut("abc\tdef", 4), "abc?...");
        assert_eq!(cut("abc", 3), "abc");
        let mut headers = HashMap::new();
        for n in 0..14 {
            headers.insert(format!("h{n:02}"), "v".to_string());
        }
        headers.insert("cseq".into(), "9".repeat(100));
        let named = headers_named(&headers);
        assert!(named.starts_with(&format!("headers cseq: {}...", "9".repeat(64))));
        assert!(named.ends_with("h10 and 3 more"));
        assert_eq!(headers_named(&HashMap::new()), "no headers");
    }

    #[test]
    fn notes_stop_at_their_limit() {
        let mut count = 0;
        let printed = (0..20).filter(|_| note(&mut count, CHANNEL_NOTES)).count();
        assert_eq!(printed, CHANNEL_NOTES.0 as usize);
        assert_eq!(count, 20);
    }
}
