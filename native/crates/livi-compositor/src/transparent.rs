//! Where a client says its surface is fully transparent (protocol livi-transparent-region-v1, native/protocols): the
//! counterpart of the opaque region. Double-buffered like it, so the region always belongs to the buffer it came with:
//! a commit brings both at once. Offered only with LIVI_COMPOSITOR_UI_HOLE (`levers`), which uses it (`hole`).

use std::sync::atomic::{AtomicBool, Ordering};

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
};
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::{
    Cacheable, RectangleKind, RegionAttributes, get_region_attributes, with_states,
};

use crate::state::LiviState;

#[allow(non_upper_case_globals, non_camel_case_types, unused_imports, missing_docs, clippy::all)]
pub mod protocol {
    use smithay::reexports::wayland_server;
    use smithay::reexports::wayland_server::protocol::*;

    pub mod __interfaces {
        use smithay::reexports::wayland_server::backend as wayland_backend;
        use smithay::reexports::wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("../../protocols/livi-transparent-region-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("../../protocols/livi-transparent-region-v1.xml");
}

use protocol::livi_transparent_region_manager_v1::{
    self as manager, LiviTransparentRegionManagerV1,
};
use protocol::livi_transparent_region_v1::{self as region, LiviTransparentRegionV1};

/// The state of a surface: None, or where it is transparent, as rectangles that do not overlap
#[derive(Default, Clone, Debug, PartialEq)]
pub struct TransparentRegion(Option<Vec<Rectangle<i32, Logical>>>);

impl Cacheable for TransparentRegion {
    // the region stays until the client sets another one, as the opaque region does
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        self.clone()
    }
    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}

/// A surface has its object: a second one is a protocol error
#[derive(Default)]
struct Taken(AtomicBool);

/// Offers the global to clients
pub fn init(dh: &DisplayHandle) {
    dh.create_global::<LiviState, LiviTransparentRegionManagerV1, ()>(1, ());
    log::info!("[hole] transparent regions offered");
}

/// Where the surface's buffer on screen is fully transparent, from the surface's corner; empty when the client said
/// nothing
pub fn rects(surface: &WlSurface) -> Vec<Rectangle<i32, Logical>> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<TransparentRegion>()
            .current()
            .0
            .clone()
            .unwrap_or_default()
    })
}

/// A wl_region as rectangles that do not overlap: added ones and taken away ones in their order
pub fn disjoint(region: &RegionAttributes) -> Vec<Rectangle<i32, Logical>> {
    let mut out: Vec<Rectangle<i32, Logical>> = Vec::new();
    for (kind, rect) in &region.rects {
        if rect.is_empty() {
            continue;
        }
        match kind {
            RectangleKind::Add => {
                let new = rect.subtract_rects(out.iter().copied());
                out.extend(new);
            }
            RectangleKind::Subtract => {
                out = Rectangle::subtract_rects_many(out, [*rect]);
            }
        }
        out.retain(|r| !r.is_empty());
    }
    out
}

fn taken(surface: &WlSurface, now: Option<bool>) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe(Taken::default);
        let flag = &states.data_map.get::<Taken>().unwrap().0;
        match now {
            Some(value) => flag.swap(value, Ordering::Relaxed),
            None => flag.load(Ordering::Relaxed),
        }
    })
}

fn set(surface: &WlSurface, rects: Option<Vec<Rectangle<i32, Logical>>>) {
    with_states(surface, |states| {
        states.cached_state.get::<TransparentRegion>().pending().0 = rects;
    });
}

impl GlobalDispatch<LiviTransparentRegionManagerV1, ()> for LiviState {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<LiviTransparentRegionManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<LiviTransparentRegionManagerV1, ()> for LiviState {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &LiviTransparentRegionManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::GetTransparentRegion { id, surface } => {
                if taken(&surface, Some(true)) {
                    resource.post_error(
                        manager::Error::AlreadyExists,
                        "the surface has a transparent region object already",
                    );
                    return;
                }
                data_init.init(id, surface.downgrade());
            }
            manager::Request::Destroy => {}
        }
    }
}

impl Dispatch<LiviTransparentRegionV1, Weak<WlSurface>> for LiviState {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &LiviTransparentRegionV1,
        request: region::Request,
        surface: &Weak<WlSurface>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Ok(surface) = surface.upgrade() else {
            return;
        };
        match request {
            region::Request::SetRegion { region } => {
                set(&surface, region.as_ref().map(|r| disjoint(&get_region_attributes(r))));
            }
            region::Request::Destroy => {
                set(&surface, None);
                taken(&surface, Some(false));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    fn area(rects: &[Rectangle<i32, Logical>]) -> i64 {
        rects
            .iter()
            .map(|r| i64::from(r.size.w) * i64::from(r.size.h))
            .sum()
    }

    #[test]
    fn a_region_becomes_rectangles_that_do_not_overlap() {
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, rect(0, 10, 100, 80)),
                (RectangleKind::Add, rect(10, 0, 80, 100)),
                (RectangleKind::Subtract, rect(40, 40, 20, 20)),
                (RectangleKind::Add, rect(0, 0, 0, 50)),
            ],
        };
        let rects = disjoint(&region);
        // a cross without its middle
        assert_eq!(area(&rects), 100 * 80 + 2 * 80 * 10 - 20 * 20);
        for (i, a) in rects.iter().enumerate() {
            assert!(!a.is_empty());
            for b in &rects[i + 1..] {
                assert!(a.intersection(*b).is_none(), "{a:?} {b:?}");
            }
            assert!(!a.overlaps(rect(40, 40, 20, 20)));
        }
    }

    #[test]
    fn a_region_taken_away_whole_is_empty() {
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, rect(0, 0, 100, 100)),
                (RectangleKind::Subtract, rect(-10, -10, 200, 200)),
            ],
        };
        assert!(disjoint(&region).is_empty());
    }
}
