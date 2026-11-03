//! Lever "UI hole" (LIVI_COMPOSITOR_UI_HOLE in LIVI's compositor): the compositor blends the window over the phone's or
//! the camera's video with each video frame, although the window draws nothing over most of it. Here the window says
//! where it is sure to be fully transparent, and the compositor leaves it out there.
//!
//! Where: the card the video shows in (app.slint, `clear-*`), without its round corners and the rim a background draws,
//! as far as the pages' place goes, and without what lies on it: LIVI's own overlays and what the plugin's components
//! report (`covers`, docs/plugins.md). When in doubt, nowhere.
//!
//! When: after Slint has drawn a frame and before it hands the frame over, from the same state the frame shows, over
//! LIVI's own Wayland protocol (native/protocols/livi-transparent-region-v1.xml). The region is double-buffered: it
//! applies with the commit of that frame, never a frame early or late. Read there, outside of what Slint tracks for its
//! redraws: a change of the covers alone (a plugin binding that reads a value changing ten times a second) brings no
//! frame. Without a new frame the old region still describes the buffer on screen.
//!
//! `LIVI_UI_HOLE_CHECK=1` reads each frame back after drawing it and logs every pixel of the region that is not clear
//! (costly: for the preview and a one-off check), also without the compositor.

use crate::AppWindow;

/// A rectangle in the window's logical pixels
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Area {
    fn right(&self) -> f32 {
        self.x + self.w
    }
    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// A rectangle in whole pixels of the surface
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Px {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Px {
    fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    fn area(&self) -> i64 {
        i64::from(self.w.max(0)) * i64::from(self.h.max(0))
    }
    fn intersects(&self, o: &Px) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
    /// What is left of this one without `o`: up to four
    fn minus(&self, o: &Px) -> Vec<Px> {
        if !self.intersects(o) {
            return vec![*self];
        }
        let (x0, y0, x1, y1) = (self.x, self.y, self.x + self.w, self.y + self.h);
        let (ox0, oy0, ox1, oy1) = (o.x.max(x0), o.y.max(y0), (o.x + o.w).min(x1), (o.y + o.h).min(y1));
        [
            Px { x: x0, y: y0, w: x1 - x0, h: oy0 - y0 },
            Px { x: x0, y: oy1, w: x1 - x0, h: y1 - oy1 },
            Px { x: x0, y: oy0, w: ox0 - x0, h: oy1 - oy0 },
            Px { x: ox1, y: oy0, w: x1 - ox1, h: oy1 - oy0 },
        ]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect()
    }
}

/// How far inside the card's edges the region begins: a background draws the card's rim there
const EDGE: f32 = 2.0;
/// Around what lies on the card, for its rounding to whole pixels
const MARGIN: f32 = 1.0;
/// A smaller part does not pay for the strips the compositor draws around it
const MIN_SIDE: i32 = 32;
/// More parts than this neither: the smallest go
const MAX_RECTS: usize = 12;

/// Where the window is fully transparent: the card without its corners (as round as `radius`) and its rim, within the
/// pages' place, without what covers it; in whole pixels of the surface (`scale` of them to a logical pixel).
pub fn clear_rects(card: Area, radius: f32, pages: Area, covers: &[Area], scale: f32) -> Vec<Px> {
    if !(scale > 0.0) || card.w <= 2.0 * EDGE || card.h <= 2.0 * EDGE {
        return Vec::new();
    }
    let (x0, y0, x1, y1) = (card.x + EDGE, card.y + EDGE, card.right() - EDGE, card.bottom() - EDGE);
    let r = radius.max(0.0).ceil();
    // a cross: the corners' squares stay out, the round edge lies within them
    let cross = [
        (x0 + r, y0, x1 - r, y1),
        (x0, y0 + r, x1, y1 - r),
    ];
    // inwards to whole pixels, within the pages' place
    let inward = |(a, b, c, d): (f32, f32, f32, f32)| {
        let a = (a.max(pages.x) * scale).ceil() as i32;
        let b = (b.max(pages.y) * scale).ceil() as i32;
        let c = (c.min(pages.right()) * scale).floor() as i32;
        let d = (d.min(pages.bottom()) * scale).floor() as i32;
        Px { x: a, y: b, w: c - a, h: d - b }
    };
    let mut rects: Vec<Px> = Vec::new();
    for part in cross.map(inward) {
        if part.is_empty() {
            continue;
        }
        // the second arm without the first: the parts do not overlap
        let mut new = vec![part];
        for had in &rects {
            new = new.iter().flat_map(|n| n.minus(had)).collect();
        }
        rects.extend(new);
    }
    // what lies on the card, outwards to whole pixels
    for c in covers.iter().filter(|c| c.w > 0.0 && c.h > 0.0) {
        let a = ((c.x - MARGIN) * scale).floor() as i32;
        let b = ((c.y - MARGIN) * scale).floor() as i32;
        let cover = Px {
            x: a,
            y: b,
            w: ((c.right() + MARGIN) * scale).ceil() as i32 - a,
            h: ((c.bottom() + MARGIN) * scale).ceil() as i32 - b,
        };
        rects = rects.iter().flat_map(|r| r.minus(&cover)).collect();
    }
    rects.retain(|r| r.w >= MIN_SIDE && r.h >= MIN_SIDE);
    if rects.len() > MAX_RECTS {
        rects.sort_by_key(|r| std::cmp::Reverse(r.area()));
        rects.truncate(MAX_RECTS);
    }
    rects
}

fn area_of(cover: &crate::CoverRect) -> Area {
    Area {
        x: cover.x,
        y: cover.y,
        w: cover.width,
        h: cover.height,
    }
}

/// The region for the frame just drawn; empty when in doubt. `scale`: surface pixels to a logical pixel
pub fn region(ui: &AppWindow, scale: f32) -> Vec<Px> {
    if !ui.get_clear_sure() {
        return Vec::new();
    }
    let Some(plugin) = crate::plugins::covers() else {
        return Vec::new();
    };
    let own = ui.get_clear_covers();
    let covers: Vec<Area> = slint::Model::iter(&own)
        .map(|c| area_of(&c))
        .chain(plugin.iter().map(|[x, y, w, h]| Area { x: *x, y: *y, w: *w, h: *h }))
        .collect();
    clear_rects(
        area_of(&ui.get_clear_card()),
        ui.get_clear_radius(),
        area_of(&ui.get_clear_pages()),
        &covers,
        scale,
    )
}

fn describe(rects: &[Px]) -> String {
    if rects.is_empty() {
        return "none".into();
    }
    let pixels: i64 = rects.iter().map(Px::area).sum();
    let list: Vec<String> = rects
        .iter()
        .map(|r| format!("{},{} {}x{}", r.x, r.y, r.w, r.h))
        .collect();
    format!("{pixels} px in {}: {}", rects.len(), list.join(", "))
}

/// Starts telling the compositor, once the window has its native surface; nothing when the compositor does not offer
/// the protocol and no check is wanted
#[cfg(target_os = "linux")]
pub fn start(ui: &AppWindow) {
    use std::cell::RefCell;

    use slint::ComponentHandle;

    let check = std::env::var("LIVI_UI_HOLE_CHECK").as_deref() == Ok("1");
    let link = wayland::Link::new(ui.window());
    if link.is_none() && !check {
        return;
    }
    println!(
        "[ui] hole: {}{}",
        if link.is_some() { "telling the compositor" } else { "no compositor to tell" },
        if check { ", checking each frame" } else { "" }
    );
    struct Run {
        link: Option<wayland::Link>,
        check: Option<check::Check>,
        last: Option<Vec<Px>>,
    }
    let run = RefCell::new(Run {
        link,
        check: check.then(check::Check::default),
        last: None,
    });
    let weak = ui.as_weak();
    let set = ui.window().set_rendering_notifier(move |state, api| {
        let Some(ui) = weak.upgrade() else { return };
        let mut run = run.borrow_mut();
        let run = &mut *run;
        if !matches!(state, slint::RenderingState::AfterRendering) {
            return;
        }
        let scale = crate::surface_scale(ui.window());
        let rects = scale.map(|scale| region(&ui, scale)).unwrap_or_default();
        if run.last.as_ref() != Some(&rects) {
            println!("[ui] hole: {}", describe(&rects));
            if let Some(link) = run.link.as_mut() {
                link.send(&rects);
            }
            run.last = Some(rects.clone());
        }
        // the surface's pixels are the framebuffer's (no system scale): the check reads where the region is
        let physical = scale.is_some_and(|s| (s - ui.window().scale_factor()).abs() < 0.001);
        if let (Some(check), slint::GraphicsAPI::NativeOpenGL { get_proc_address }, true) =
            (run.check.as_mut(), api, physical)
        {
            let height = ui.window().size().height as i32;
            check.frame(get_proc_address, height, &rects);
        }
    });
    if let Err(e) = set {
        eprintln!("[ui] hole: no rendering notifier: {e}");
    }
}

#[cfg(not(target_os = "linux"))]
pub fn start(_: &AppWindow) {}

/// LIVI_UI_HOLE_CHECK: what the region claims, read back from the frame just drawn
#[cfg(target_os = "linux")]
mod check {
    use std::ffi::{CStr, c_void};

    use super::Px;

    const GL_RGBA: u32 = 0x1908;
    const GL_UNSIGNED_BYTE: u32 = 0x1401;
    const GL_PACK_ALIGNMENT: u32 = 0x0D05;
    const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

    type ReadPixels = unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void);
    type GetIntegerv = unsafe extern "C" fn(u32, *mut i32);
    type PixelStorei = unsafe extern "C" fn(u32, i32);

    #[derive(Default)]
    pub struct Check {
        frames: u64,
        clear: u64,
    }

    impl Check {
        pub fn frame(&mut self, get: &dyn Fn(&CStr) -> *const c_void, height: i32, rects: &[Px]) {
            let (read, integer, store) = (get(c"glReadPixels"), get(c"glGetIntegerv"), get(c"glPixelStorei"));
            if read.is_null() || integer.is_null() || store.is_null() {
                return;
            }
            // SAFETY: the pointers come from the context that is current while Slint draws, with GL's signatures
            let (read, integer, store): (ReadPixels, GetIntegerv, PixelStorei) =
                unsafe { (std::mem::transmute(read), std::mem::transmute(integer), std::mem::transmute(store)) };
            let mut bound = -1;
            let mut alignment = 4;
            unsafe {
                integer(GL_FRAMEBUFFER_BINDING, &mut bound);
                integer(GL_PACK_ALIGNMENT, &mut alignment);
            }
            // the window's own framebuffer only
            if bound != 0 {
                return;
            }
            self.frames += 1;
            let mut wrong = 0_u64;
            let mut first = None;
            unsafe { store(GL_PACK_ALIGNMENT, 4) };
            for r in rects {
                let mut pixels = vec![0_u8; r.w as usize * r.h as usize * 4];
                // GL counts rows from the bottom
                unsafe {
                    read(r.x, height - r.y - r.h, r.w, r.h, GL_RGBA, GL_UNSIGNED_BYTE, pixels.as_mut_ptr().cast());
                }
                for (i, p) in pixels.chunks_exact(4).enumerate() {
                    if p != [0, 0, 0, 0] {
                        wrong += 1;
                        if first.is_none() {
                            let (column, row) = (i as i32 % r.w, i as i32 / r.w);
                            first = Some((r.x + column, r.y + r.h - 1 - row, [p[0], p[1], p[2], p[3]]));
                        }
                    }
                }
            }
            unsafe { store(GL_PACK_ALIGNMENT, alignment) };
            if let Some((x, y, rgba)) = first {
                println!("[ui] hole check: frame {}: {wrong} pixels not clear, first at {x},{y} {rgba:?}", self.frames);
            } else if !rects.is_empty() {
                self.clear += 1;
            }
            if self.frames.is_multiple_of(100) {
                println!("[ui] hole check: {} frames, {} with a region all clear", self.frames, self.clear);
            }
        }
    }
}

/// The protocol's client side, on the connection winit opened for the window
#[cfg(target_os = "linux")]
mod wayland {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use wayland_client::backend::{Backend, ObjectId};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_compositor::WlCompositor;
    use wayland_client::protocol::wl_region::WlRegion;
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};

    use super::Px;

    #[allow(non_upper_case_globals, non_camel_case_types, unused_imports, missing_docs, clippy::all)]
    pub mod protocol {
        use wayland_client;
        use wayland_client::protocol::*;

        pub mod __interfaces {
            use wayland_client::backend as wayland_backend;
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("../../protocols/livi-transparent-region-v1.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_client_code!("../../protocols/livi-transparent-region-v1.xml");
    }

    use protocol::livi_transparent_region_manager_v1::LiviTransparentRegionManagerV1;
    use protocol::livi_transparent_region_v1::LiviTransparentRegionV1;

    pub struct Globals;

    impl Dispatch<WlRegistry, GlobalListContents> for Globals {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    delegate_noop!(Globals: WlCompositor);
    delegate_noop!(Globals: WlRegion);
    delegate_noop!(Globals: LiviTransparentRegionManagerV1);
    delegate_noop!(Globals: LiviTransparentRegionV1);

    pub struct Link {
        conn: Connection,
        queue: EventQueue<Globals>,
        compositor: WlCompositor,
        region: LiviTransparentRegionV1,
    }

    impl Link {
        /// The window's surface with its transparent region; None when the compositor does not offer it
        pub fn new(window: &slint::Window) -> Option<Self> {
            let handle = window.window_handle();
            let RawDisplayHandle::Wayland(display) = handle.display_handle().ok()?.as_raw() else {
                return None;
            };
            let RawWindowHandle::Wayland(surface) = handle.window_handle().ok()?.as_raw() else {
                return None;
            };
            // SAFETY: winit's display and surface, which live as long as the window; this connection does not own them
            let conn = Connection::from_backend(unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) });
            let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.surface.as_ptr().cast()) }.ok()?;
            let surface = WlSurface::from_id(&conn, id).ok()?;
            let (globals, queue) = registry_queue_init::<Globals>(&conn).ok()?;
            let qh = queue.handle();
            let manager: LiviTransparentRegionManagerV1 = globals.bind(&qh, 1..=1, ()).ok()?;
            let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).ok()?;
            let region = manager.get_transparent_region(&surface, &qh, ());
            manager.destroy();
            Some(Self {
                conn,
                queue,
                compositor,
                region,
            })
        }

        /// The region for the next commit of the surface, the frame about to be drawn
        pub fn send(&mut self, rects: &[Px]) {
            let qh = self.queue.handle();
            if rects.is_empty() {
                self.region.set_region(None);
            } else {
                let region = self.compositor.create_region(&qh, ());
                for r in rects {
                    region.add(r.x, r.y, r.w, r.h);
                }
                self.region.set_region(Some(&region));
                region.destroy();
            }
            // goes out with the commit at the latest; flushing now keeps the order all the same
            let _ = self.conn.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: Area = Area { x: 300.0, y: 16.0, w: 2036.0, h: 742.0 };
    const PAGES: Area = Area { x: 290.0, y: 0.0, w: 2110.0, h: 770.0 };

    fn total(rects: &[Px]) -> i64 {
        rects.iter().map(Px::area).sum()
    }

    fn disjoint(rects: &[Px]) {
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!a.intersects(b), "{a:?} {b:?}");
            }
        }
    }

    /// Inside the card, 2 px from its edges, not in a corner's square
    fn inside(rects: &[Px], card: Area, radius: f32) {
        for r in rects {
            assert!(r.x as f32 >= card.x + EDGE && r.y as f32 >= card.y + EDGE, "{r:?}");
            assert!((r.x + r.w) as f32 <= card.right() - EDGE && (r.y + r.h) as f32 <= card.bottom() - EDGE, "{r:?}");
            for (cx, cy) in [(card.x, card.y), (card.right() - radius, card.y), (card.x, card.bottom() - radius),
                (card.right() - radius, card.bottom() - radius)]
            {
                let corner = Px { x: cx as i32, y: cy as i32, w: radius as i32, h: radius as i32 };
                assert!(!r.intersects(&corner), "{r:?} in the corner {corner:?}");
            }
        }
    }

    #[test]
    fn an_empty_card_has_no_region() {
        let none = Area { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };
        assert!(clear_rects(none, 0.0, PAGES, &[], 1.0).is_empty());
        assert!(clear_rects(CARD, 26.0, PAGES, &[], 0.0).is_empty());
    }

    #[test]
    fn the_card_without_corners_and_rim() {
        let rects = clear_rects(CARD, 26.0, PAGES, &[], 1.0);
        disjoint(&rects);
        inside(&rects, CARD, 26.0);
        // the cross without the corners' squares and the 2 px rim; its arms beside the corners are 26 px wide, too narrow
        // to pay (MIN_SIDE): the middle is left
        let (w, h) = (2036 - 4, 742 - 4);
        assert_eq!(rects.len(), 1);
        assert_eq!(total(&rects), i64::from((w - 2 * 26) * h));
        // a rounder card keeps its arms
        let rects = clear_rects(CARD, 40.0, PAGES, &[], 1.0);
        assert_eq!(rects.len(), 3);
        assert_eq!(total(&rects), i64::from(w * h - 4 * 40 * 40));
    }

    #[test]
    fn fractions_go_inwards_and_the_pages_bound_it() {
        // the card reaches over the pages' edge at 290: that edge bounds it, the card's own 2 px and fractions go inwards
        let card = Area { x: 280.4, y: 15.6, w: 2000.3, h: 700.2 };
        let rects = clear_rects(card, 0.0, PAGES, &[], 1.0);
        assert_eq!(rects, vec![Px { x: 290, y: 18, w: 1988, h: 695 }]);
    }

    #[test]
    fn what_lies_on_the_card_comes_off_with_a_pixel_around() {
        let button = Area { x: 2244.0, y: 36.0, w: 72.0, h: 72.0 };
        let rects = clear_rects(CARD, 26.0, PAGES, &[button], 1.0);
        disjoint(&rects);
        inside(&rects, CARD, 26.0);
        let around = Px { x: 2243, y: 35, w: 74, h: 74 };
        assert!(rects.iter().all(|r| !r.intersects(&around)));
        let whole = total(&clear_rects(CARD, 26.0, PAGES, &[], 1.0));
        // what goes: the button and the strip beside it too narrow to pay (2316 + 1 to the right arm's start)
        assert!(total(&rects) < whole - 74 * 74);
        assert!(total(&rects) > whole - 200 * 200);
    }

    #[test]
    fn a_cover_over_all_of_it_leaves_nothing() {
        let all = Area { x: 0.0, y: 0.0, w: 2400.0, h: 900.0 };
        assert!(clear_rects(CARD, 26.0, PAGES, &[all], 1.0).is_empty());
    }

    #[test]
    fn a_zoomed_window_counts_in_the_surface_pixels() {
        let rects = clear_rects(CARD, 0.0, PAGES, &[], 1.5);
        assert_eq!(rects.len(), 1);
        let r = rects[0];
        assert_eq!((r.x, r.y), (453, 27));
        assert_eq!(r.x + r.w, ((2336.0 - EDGE) * 1.5) as i32);
    }

    #[test]
    fn many_covers_keep_only_the_largest_parts() {
        let covers: Vec<Area> = (0..10)
            .map(|i| Area { x: 400.0 + 180.0 * i as f32, y: 100.0 + 50.0 * (i % 3) as f32, w: 60.0, h: 60.0 })
            .collect();
        let rects = clear_rects(CARD, 26.0, PAGES, &covers, 1.0);
        assert!(!rects.is_empty() && rects.len() <= MAX_RECTS);
        disjoint(&rects);
        for c in &covers {
            let px = Px { x: c.x as i32 - 1, y: c.y as i32 - 1, w: c.w as i32 + 2, h: c.h as i32 + 2 };
            assert!(rects.iter().all(|r| !r.intersects(&px)));
        }
    }
}
