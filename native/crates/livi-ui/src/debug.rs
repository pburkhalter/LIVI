//! Debugging aids: timings of the start (`LIVI_UI_TRACE=1`: milliseconds since the start of the process for each step
//! and the first frames) and the debug overlay (setting "debugOverlay" or `LIVI_DEBUG_OVERLAY=1`): a few lines about
//! the running system in a corner of the main screen, refreshed once a second.

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static CORE_VERSION: Mutex<String> = Mutex::new(String::new());

thread_local! {
    static FRAMES: Cell<u64> = const { Cell::new(0) };
    static SAMPLE: RefCell<Option<Sample>> = const { RefCell::new(None) };
}

pub fn start() {
    START.get_or_init(Instant::now);
}

fn tracing() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LIVI_UI_TRACE").is_some_and(|v| v != "0"))
}

pub fn since_start_ms() -> u128 {
    START.get().map_or(0, |s| s.elapsed().as_millis())
}

/// One step of the start, printed with LIVI_UI_TRACE
pub fn trace(what: &str) {
    if tracing() {
        println!("[ui] +{} ms {what}", since_start_ms());
    }
}

/// Called after every rendered frame; traces the first ones.
pub fn frame_rendered() {
    let n = FRAMES.get() + 1;
    FRAMES.set(n);
    if n <= 5 || n == 30 || n == 120 {
        trace(&format!("frame {n} rendered"));
    }
}

pub fn frames() -> u64 {
    FRAMES.get()
}

/// Version of LIVI's core, from its welcome (called on the client's thread)
pub fn set_core_version(version: &str) {
    if let Ok(mut v) = CORE_VERSION.lock() {
        *v = version.to_string();
    }
}

/// Whether the overlay is wanted: the setting, or LIVI_DEBUG_OVERLAY for a run
pub fn overlay_wanted(config: &serde_json::Value) -> bool {
    static FORCED: OnceLock<bool> = OnceLock::new();
    *FORCED.get_or_init(|| std::env::var_os("LIVI_DEBUG_OVERLAY").is_some_and(|v| v != "0"))
        || config["debugOverlay"].as_bool().unwrap_or(false)
}

struct Sample {
    at: Instant,
    frames: u64,
    cpu: Option<(u64, u64)>,
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// (busy, total) jiffies of all CPUs
fn cpu_times() -> Option<(u64, u64)> {
    let stat = read("/proc/stat")?;
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    let v: Vec<u64> = line
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let total: u64 = v.iter().take(8).sum();
    let idle = v.get(3)? + v.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

fn kb_field(text: &str, key: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with(key))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn max_temp_c() -> Option<f64> {
    let zones = std::fs::read_dir("/sys/class/thermal").ok()?;
    zones
        .filter_map(|z| read(&format!("{}/temp", z.ok()?.path().display())))
        .filter_map(|t| t.trim().parse::<f64>().ok())
        .map(|m| m / 1000.0)
        .reduce(f64::max)
}

fn cpu_mhz() -> Option<u64> {
    let khz: u64 = read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")?
        .trim()
        .parse()
        .ok()?;
    Some(khz / 1000)
}

/// The overlay's lines: what changes since the last call (frame rate, CPU load) and what is now.
pub fn overlay_text(page: &str, phone: &str, plugin: Option<String>) -> String {
    let now = Instant::now();
    let frames = frames();
    let cpu = cpu_times();
    let (fps, load) = SAMPLE.with_borrow_mut(|last| {
        let out = last.as_ref().map_or((None, None), |l| {
            let secs = now.duration_since(l.at).as_secs_f64().max(0.001);
            let fps = (frames - l.frames) as f64 / secs;
            let load = match (l.cpu, cpu) {
                (Some((b0, t0)), Some((b1, t1))) if t1 > t0 => {
                    Some(100.0 * (b1 - b0) as f64 / (t1 - t0) as f64)
                }
                _ => None,
            };
            (Some(fps), load)
        });
        *last = Some(Sample {
            at: now,
            frames,
            cpu,
        });
        out
    });
    let dash = || "–".to_string();
    let version = CORE_VERSION.lock().map(|v| v.clone()).unwrap_or_default();
    let mem = read("/proc/meminfo");
    let free = mem
        .as_deref()
        .and_then(|m| kb_field(m, "MemAvailable:"))
        .map_or_else(dash, |kb| format!("{:.1} GB", kb as f64 / 1_048_576.0));
    let rss = read("/proc/self/status")
        .as_deref()
        .and_then(|s| kb_field(s, "VmRSS:"))
        .map_or_else(dash, |kb| format!("{} MB", kb / 1024));
    format!(
        "LIVI {} · UI {} fps · CPU {}{} · {}\nRAM free {} · UI {}{}\n{} · phone {}",
        if version.is_empty() { "–" } else { &version },
        fps.map_or_else(dash, |f| format!("{f:.0}")),
        load.map_or_else(dash, |l| format!("{l:.0} %")),
        cpu_mhz().map_or_else(String::new, |m| format!(" @ {m} MHz")),
        max_temp_c().map_or_else(dash, |t| format!("{t:.0} °C")),
        free,
        rss,
        plugin.map_or_else(String::new, |p| format!(" · plugin {p}")),
        page,
        phone,
    )
}
