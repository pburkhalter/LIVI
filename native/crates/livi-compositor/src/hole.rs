//! Lever LIVI_COMPOSITOR_UI_HOLE (`levers`): each video frame blends LIVI's UI over the whole video, although the UI
//! draws nothing over most of it. The UI says where it is fully transparent (`transparent`), and the UI is drawn only
//! around that part of the video.
//!
//! Only the UI knows what lies on its card: its own overlays and what a plugin draws there and reports
//! (docs/plugins.md). Its region rides on the commit of the buffer it belongs to, so it is never a frame early or late,
//! and a UI that says nothing is drawn whole, as without the lever.

use smithay::utils::{Logical, Physical, Point, Rectangle, Size};
use smithay::wayland::compositor::get_children;

use crate::render::surface_size;
use crate::state::{Kind, LiviState};

/// A part of the hole narrower or lower than this does not pay for the strips around it
const MIN_SIDE: i32 = 64;
/// Nor does a hole smaller than this
const MIN_PIXELS: i64 = 256 * 256;
/// More strips than this cost more draws than they save
const MAX_STRIPS: usize = 24;

/// What is left out of a UI window, one for each
#[derive(Default)]
pub struct Hole {
    window: Size<i32, Logical>,
    /// From the window's corner, not overlapping
    holes: Vec<Rectangle<i32, Logical>>,
    log: Pace,
}

/// Lines of the region's changes a second at most, except a change to or from none (a plugin's cover that follows an
/// animation would write one a frame)
const LINES_A_SECOND: u32 = 4;

/// How the region's changes reach the log: each one to or from none, of the others LINES_A_SECOND a second; the next
/// line says how many it left out
#[derive(Default)]
pub(crate) struct Pace {
    since: Option<std::time::Instant>,
    lines: u32,
    left_out: u32,
}

impl Pace {
    pub(crate) fn line(&mut self, edge: bool, text: impl FnOnce() -> String) -> Option<String> {
        let now = std::time::Instant::now();
        if self.since.is_none_or(|s| now.duration_since(s).as_secs() >= 1) {
            self.since = Some(now);
            self.lines = 0;
        }
        if !edge && self.lines >= LINES_A_SECOND {
            self.left_out += 1;
            return None;
        }
        self.lines += 1;
        Some(match std::mem::take(&mut self.left_out) {
            0 => text(),
            n => format!("{} ({n} changes before not logged)", text()),
        })
    }
}

impl Hole {
    /// The parts of the window to draw, for a window at `at`; None: all of it as usual
    pub fn strips(&self, at: Point<i32, Physical>) -> Option<Vec<Rectangle<i32, Physical>>> {
        around(at, self.window, &self.holes)
    }
}

/// The window without the holes, as rectangles at `at`; None without a hole or when it would take too many
pub fn around(
    at: Point<i32, Physical>,
    window: Size<i32, Logical>,
    holes: &[Rectangle<i32, Logical>],
) -> Option<Vec<Rectangle<i32, Physical>>> {
    if holes.is_empty() {
        return None;
    }
    let strips = Rectangle::from_size(window).subtract_rects(holes.iter().copied());
    (strips.len() <= MAX_STRIPS).then(|| {
        strips
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| {
                Rectangle::new(
                    (at.x + s.loc.x, at.y + s.loc.y).into(),
                    (s.size.w, s.size.h).into(),
                )
            })
            .collect()
    })
}

/// What to leave out of a window of this size: where it is transparent over a video, without parts too small to pay
pub fn holes_in(
    transparent: &[Rectangle<i32, Logical>],
    window: Size<i32, Logical>,
    video: Option<Rectangle<i32, Logical>>,
) -> Vec<Rectangle<i32, Logical>> {
    let Some(video) = video else {
        return Vec::new();
    };
    let window = Rectangle::from_size(window);
    let holes: Vec<_> = transparent
        .iter()
        .filter_map(|r| r.intersection(window)?.intersection(video))
        .filter(|r| r.size.w >= MIN_SIDE && r.size.h >= MIN_SIDE)
        .collect();
    let pixels: i64 = holes
        .iter()
        .map(|r| i64::from(r.size.w) * i64::from(r.size.h))
        .sum();
    if pixels < MIN_PIXELS {
        return Vec::new();
    }
    holes
}

/// Brings what is left out of the UI windows on this screen up to date, before a frame is put together: from the
/// region committed with the buffer on screen
pub fn refresh(state: &mut LiviState, screen_idx: usize) {
    for ui in 0..state.toplevels.len() {
        let t = &state.toplevels[ui];
        if t.kind != Kind::Ui || t.screen_idx != screen_idx {
            continue;
        }
        let surface = t.toplevel.wl_surface();
        let window: Size<i32, Logical> = surface_size(surface).into();
        // a subsurface would be cut as well: it may draw where its parent is transparent
        let holes = if get_children(surface).is_empty() {
            holes_in(
                &crate::transparent::rects(surface),
                window,
                video_area(state, ui),
            )
        } else {
            Vec::new()
        };
        let hole = &mut state.toplevels[ui].hole;
        if hole.holes != holes
            && let Some(line) = hole
                .log
                .line(hole.holes.is_empty() || holes.is_empty(), || describe(&holes))
        {
            log::info!("[hole] {line}");
        }
        hole.window = window;
        hole.holes = holes;
    }
}

fn describe(holes: &[Rectangle<i32, Logical>]) -> String {
    if holes.is_empty() {
        return "none".into();
    }
    let pixels: i64 = holes
        .iter()
        .map(|r| i64::from(r.size.w) * i64::from(r.size.h))
        .sum();
    let list: Vec<String> = holes
        .iter()
        .map(|r| format!("{},{} {}x{}", r.loc.x, r.loc.y, r.size.w, r.size.h))
        .collect();
    format!("{} px in {}: {}", pixels, holes.len(), list.join(", "))
}

/// What the visible videos of the window's screen cover of it, from its corner
fn video_area(state: &LiviState, ui: usize) -> Option<Rectangle<i32, Logical>> {
    let t = &state.toplevels[ui];
    let size: Size<i32, Logical> = surface_size(t.toplevel.wl_surface()).into();
    let window = Rectangle::new(t.position, size);
    let shown = state
        .toplevels
        .iter()
        .filter(|v| v.kind == Kind::Video && v.screen_idx == t.screen_idx && v.visible)
        .map(|v| Rectangle::new(v.position, surface_size(v.toplevel.wl_surface()).into()))
        .filter_map(|v| v.intersection(window))
        .reduce(|a, b| a.merge(b))?;
    let area = Rectangle::new(
        (shown.loc.x - window.loc.x, shown.loc.y - window.loc.y).into(),
        shown.size,
    );
    (!area.is_empty()).then_some(area)
}

#[cfg(test)]
mod tests;
