//! Lever LIVI_COMPOSITOR_UI_HOLE (`levers`): each video frame blends LIVI's UI over the whole video, although most of it
//! is fully transparent there. This reads the UI's buffer once it has stood for a moment, finds the largest rectangle
//! of zero pixels over the video and draws the UI only around it.
//!
//! The rectangle comes from the pixels, not from what the UI says about its card: LIVI and a plugin draw on the card
//! too (the level and phone overlays, a plugin's notices, a keyboard), and a pixel of zero leaves what lies below it
//! unchanged, whatever drew it. A commit ends the rectangle before the frame that shows it, so the UI is whole until
//! its new buffer has been read.

use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesError, GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::{CommitCounter, with_renderer_surface_state};
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen, Renderer, Texture};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{
    Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Size, Transform,
};
use smithay::wayland::compositor::get_children;

use crate::render::surface_size;
use crate::state::{Kind, LiviState};

/// How long a buffer stands before it is read: a UI that keeps drawing is not read for each of its frames
const SETTLE: Duration = Duration::from_millis(500);
/// Side of the squares the buffer is judged in: a square counts as clear when all of its pixels are
const TILE: usize = 8;
/// A smaller rectangle does not pay for the three more draws around it
const MIN_PIXELS: usize = 256 * 256;

/// What is known about the UI's buffer, one for each UI window
#[derive(Default)]
pub struct Hole {
    commit: Option<CommitCounter>,
    since: Option<Instant>,
    /// What was read of this buffer: the video may grow without a commit of the UI
    read: Option<Rectangle<i32, Logical>>,
    cut: Option<Cut>,
}

#[derive(Clone, Copy)]
struct Cut {
    window: Size<i32, Logical>,
    /// From the window's corner
    hole: Rectangle<i32, Logical>,
}

impl Hole {
    /// True when the buffer has stood long enough to be read and this area of it was not read yet
    fn due(&mut self, commit: CommitCounter, area: Rectangle<i32, Logical>, now: Instant) -> bool {
        if self.note(commit, now) {
            return false;
        }
        self.read != Some(area)
            && self
                .since
                .is_some_and(|since| now.duration_since(since) >= SETTLE)
    }

    /// A new commit ends what was known about the one before; true when this one is new
    fn note(&mut self, commit: CommitCounter, now: Instant) -> bool {
        let new = self.commit != Some(commit);
        if new {
            *self = Self {
                commit: Some(commit),
                since: Some(now),
                ..Self::default()
            };
        }
        new
    }

    /// The parts of the window to draw, for a window at `at`; None: all of it as usual
    pub fn strips(&self, at: Point<i32, Physical>) -> Option<Vec<Rectangle<i32, Physical>>> {
        let Cut { window, hole } = self.cut?;
        Some(around(at, window, hole))
    }
}

fn around(
    at: Point<i32, Physical>,
    window: Size<i32, Logical>,
    hole: Rectangle<i32, Logical>,
) -> Vec<Rectangle<i32, Physical>> {
    let window = Rectangle::new(at, (window.w, window.h).into());
    let hole = Rectangle::new(
        (at.x + hole.loc.x, at.y + hole.loc.y).into(),
        (hole.size.w, hole.size.h).into(),
    );
    window.subtract_rect(hole)
}

/// Brings what is known about the UI's buffers on this screen up to date, before a frame is put together
pub fn refresh(state: &mut LiviState, screen_idx: usize) {
    let now = Instant::now();
    for ui in 0..state.toplevels.len() {
        let t = &state.toplevels[ui];
        if t.kind != Kind::Ui || t.screen_idx != screen_idx {
            continue;
        }
        let surface = t.toplevel.wl_surface().clone();
        let Some(commit) = with_renderer_surface_state(&surface, |s| s.current_commit()) else {
            continue;
        };
        // nothing to win without a video, and the next frame with one asks again
        let Some((window, area)) = video_area(state, ui) else {
            state.toplevels[ui].hole.note(commit, now);
            continue;
        };
        if !state.toplevels[ui].hole.due(commit, area, now) {
            continue;
        }
        let Some(renderer) = state.renderer.as_mut() else {
            return;
        };
        let Some(texture) = plain_texture(renderer, &surface) else {
            continue;
        };
        let started = Instant::now();
        let found = match largest_clear_in(renderer, &texture, area) {
            Ok(found) => found,
            Err(e) => {
                log::warn!("reading the UI: {e}");
                None
            }
        };
        let hole = &mut state.toplevels[ui].hole;
        hole.read = Some(area);
        hole.cut = found.map(|hole| Cut { window, hole });
        log::info!(
            "[hole] {:.1} ms, area {}x{}: {found:?}",
            started.elapsed().as_secs_f64() * 1000.0,
            area.size.w,
            area.size.h
        );
    }
}

/// The window's size and what the visible videos of its screen cover of it, from its corner
fn video_area(
    state: &LiviState,
    ui: usize,
) -> Option<(Size<i32, Logical>, Rectangle<i32, Logical>)> {
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
    (!area.is_empty()).then_some((size, area))
}

/// The surface's texture, when its pixels are the window's one to one
fn plain_texture(renderer: &GlesRenderer, surface: &WlSurface) -> Option<GlesTexture> {
    if !get_children(surface).is_empty() {
        return None;
    }
    let context = renderer.context_id();
    with_renderer_surface_state(surface, |s| {
        let plain = s.buffer_scale() == 1
            && s.buffer_transform() == Transform::Normal
            && s.buffer_size().is_some()
            && s.buffer_size() == s.surface_size();
        plain
            .then(|| s.texture::<GlesTexture>(context).cloned())
            .flatten()
    })
    .flatten()
}

/// The largest clear rectangle inside `area` of the texture, in the window's rows from its corner
pub fn largest_clear_in(
    renderer: &mut GlesRenderer,
    texture: &GlesTexture,
    area: Rectangle<i32, Logical>,
) -> Result<Option<Rectangle<i32, Logical>>, GlesError> {
    match read(renderer, texture, area) {
        // a buffer the GPU can only sample cannot be read
        Err(GlesError::FramebufferBindingError) => read_a_copy(renderer, texture, area),
        found => found,
    }
}

fn read(
    renderer: &mut GlesRenderer,
    texture: &GlesTexture,
    area: Rectangle<i32, Logical>,
) -> Result<Option<Rectangle<i32, Logical>>, GlesError> {
    // a buffer drawn upside down lies in the texture the other way round
    let inverted = texture.is_y_inverted();
    let height = texture.size().h;
    let row = if inverted {
        height - area.loc.y - area.size.h
    } else {
        area.loc.y
    };
    let region = Rectangle::<i32, BufferCoords>::new(
        (area.loc.x, row).into(),
        (area.size.w, area.size.h).into(),
    );
    let mapping = renderer.copy_texture(texture, region, Fourcc::Abgr8888)?;
    let pixels = renderer.map_texture(&mapping)?;
    let found = largest_clear(pixels, area.size.w as usize, area.size.h as usize);
    Ok(found.map(|[x, y, w, h]| {
        let y = if inverted {
            area.size.h as usize - y - h
        } else {
            y
        };
        Rectangle::new(
            (area.loc.x + x as i32, area.loc.y + y as i32).into(),
            (w as i32, h as i32).into(),
        )
    }))
}

/// The same through a texture that can be read: all of the buffer drawn into it pixel for pixel, without blending
fn read_a_copy(
    renderer: &mut GlesRenderer,
    texture: &GlesTexture,
    area: Rectangle<i32, Logical>,
) -> Result<Option<Rectangle<i32, Logical>>, GlesError> {
    let size = texture.size();
    let mut copy = Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size)?;
    {
        let all = Rectangle::<i32, Physical>::from_size((size.w, size.h).into());
        let src = Rectangle::<f64, BufferCoords>::from_size(size.to_f64());
        let mut target = renderer.bind(&mut copy)?;
        let mut frame = renderer.render(&mut target, all.size, Transform::Normal)?;
        frame.clear(Color32F::TRANSPARENT, &[all])?;
        frame.render_texture_from_to(
            texture,
            src,
            all,
            &[all],
            &[all],
            Transform::Normal,
            1.0,
            None,
            &[],
        )?;
        // the same context reads it, in order; no need to wait
        let _ = frame.finish()?;
    }
    read(renderer, &copy, area)
}

/// The largest rectangle of squares in which all four bytes of every pixel are zero (`pixels`: 4 bytes each, row after
/// row): x, y, width, height in pixels. Only a pixel of zero leaves what lies below unchanged: alpha 0 alone would not
/// be enough, and alpha 1 of 255, which LIVI draws around a card, is no hole.
fn largest_clear(pixels: &[u8], width: usize, height: usize) -> Option<[usize; 4]> {
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return None;
    }
    let columns = width.div_ceil(TILE);
    let rows = height.div_ceil(TILE);
    let mut clear = vec![true; columns * rows];
    for (y, row) in pixels.chunks_exact(width * 4).take(height).enumerate() {
        let tiles = &mut clear[y / TILE * columns..][..columns];
        for (tile, part) in tiles.iter_mut().zip(row.chunks(TILE * 4)) {
            *tile = *tile && part.iter().fold(0, |any, byte| any | byte) == 0;
        }
    }
    let mut best = [0; 4];
    let mut best_pixels = 0;
    // the classic largest rectangle in a histogram, one histogram for each row: how many clear squares end here
    let mut above = vec![0_usize; columns];
    let mut open: Vec<(usize, usize)> = Vec::with_capacity(columns + 1);
    for row in 0..rows {
        open.clear();
        for column in 0..=columns {
            let tall = if column == columns {
                0
            } else if clear[row * columns + column] {
                above[column] + 1
            } else {
                0
            };
            if column < columns {
                above[column] = tall;
            }
            let mut start = column;
            while let Some(&(left, top)) = open.last().filter(|(_, top)| *top >= tall) {
                open.pop();
                start = left;
                let x = left * TILE;
                let y = (row + 1 - top) * TILE;
                let w = (column * TILE).min(width) - x;
                let h = ((row + 1) * TILE).min(height) - y;
                if w * h > best_pixels {
                    best_pixels = w * h;
                    best = [x, y, w, h];
                }
            }
            if tall > 0 {
                open.push((start, tall));
            }
        }
    }
    (best_pixels >= MIN_PIXELS).then_some(best)
}

#[cfg(test)]
mod tests;
