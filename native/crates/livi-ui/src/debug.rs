//! Timings of the start (`LIVI_UI_TRACE=1`: milliseconds since the start of the process for each step and the first
//! frames) and the frame counter behind the debug overlay.

use std::cell::Cell;
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

thread_local! {
    static FRAMES: Cell<u64> = const { Cell::new(0) };
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
