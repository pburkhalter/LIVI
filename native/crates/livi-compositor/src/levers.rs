//! Switches for the graphics chain, read once at the start. Each is off unless its variable is set; with none set the
//! compositor paces and draws as it does without them. They are here to be measured on the device.

use std::sync::LazyLock;

use smithay::backend::renderer::{Color32F, FullDamage};

/// LIVI_COMPOSITOR_EARLY_CALLBACK=1: the frame callbacks go out when the GPU starts on a frame, not when it has finished
/// it, so that a client draws its next frame while the GPU works on this one (see `drm::render`).
pub static EARLY_CALLBACK: LazyLock<bool> = LazyLock::new(|| {
    on(std::env::var("LIVI_COMPOSITOR_EARLY_CALLBACK")
        .ok()
        .as_deref())
});

/// LIVI_COMPOSITOR_CAMERA_FIRST=1: the start's wait for the UI also ends when the camera's picture has come, so that a
/// reversing camera shows before the UI has drawn (see `drm::keep_splash`).
pub static CAMERA_FIRST: LazyLock<bool> = LazyLock::new(|| {
    on(std::env::var("LIVI_COMPOSITOR_CAMERA_FIRST")
        .ok()
        .as_deref())
});

/// LIVI_COMPOSITOR_UI_HOLE=1: the UI is not drawn over a video where it says it is fully transparent (see `hole`,
/// `transparent`); a client learns of it through the global of livi-transparent-region-v1, offered only then.
pub static UI_HOLE: LazyLock<bool> =
    LazyLock::new(|| on(std::env::var("LIVI_COMPOSITOR_UI_HOLE").ok().as_deref()));

/// LIVI_COMPOSITOR_INVALIDATE: what a frame that draws all of its target anew does first. `1` tells the GPU that the old
/// contents are not needed (`glInvalidateFramebuffer`); `clear` clears to magenta instead (`glClear`), which costs a
/// tile-based GPU as little and shows if a frame leaves any pixel out.
pub static FULL_DAMAGE: LazyLock<FullDamage> =
    LazyLock::new(|| full_damage(std::env::var("LIVI_COMPOSITOR_INVALIDATE").ok().as_deref()));

/// LIVI_COMPOSITOR_DMABUF_WAIT: what a commit with a client's dma-buf does about the buffer's implicit fences (see
/// `fence`). `1` holds the commit until they have signalled; `early` also sends the commit's frame callbacks at once, so
/// that the client can draw the next frame meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmabufWait {
    Off,
    Hold,
    Early,
}

pub static DMABUF_WAIT: LazyLock<DmabufWait> =
    LazyLock::new(|| dmabuf_wait(std::env::var("LIVI_COMPOSITOR_DMABUF_WAIT").ok().as_deref()));

fn dmabuf_wait(value: Option<&str>) -> DmabufWait {
    match value {
        Some("1") => DmabufWait::Hold,
        Some("early") => DmabufWait::Early,
        _ => DmabufWait::Off,
    }
}

fn on(value: Option<&str>) -> bool {
    value == Some("1")
}

fn full_damage(value: Option<&str>) -> FullDamage {
    match value {
        Some("1") => FullDamage::Discard,
        Some("clear") => FullDamage::Clear(Color32F::new(1.0, 0.0, 1.0, 1.0)),
        _ => FullDamage::Keep,
    }
}

/// A line in the log when a switch is on: a measurement has to show what it ran with
pub fn announce() {
    let (early, hole, full, camera, dmabuf) =
        (*EARLY_CALLBACK, *UI_HOLE, *FULL_DAMAGE, *CAMERA_FIRST, *DMABUF_WAIT);
    if early || hole || full != FullDamage::Keep || camera || dmabuf != DmabufWait::Off {
        log::info!(
            "graphics levers: early callback {early}, UI hole {hole}, full damage {full:?}, camera first {camera}, \
             dma-buf wait {dmabuf:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_variable_every_lever_is_off() {
        assert!(!on(None));
        assert_eq!(full_damage(None), FullDamage::Keep);
        assert_eq!(dmabuf_wait(None), DmabufWait::Off);
    }

    #[test]
    fn the_dmabuf_wait_knows_two_values() {
        assert_eq!(dmabuf_wait(Some("1")), DmabufWait::Hold);
        assert_eq!(dmabuf_wait(Some("early")), DmabufWait::Early);
        for other in ["", "0", "true", "yes", "2"] {
            assert_eq!(dmabuf_wait(Some(other)), DmabufWait::Off, "{other}");
        }
    }

    #[test]
    fn only_the_documented_values_switch() {
        assert!(on(Some("1")));
        for other in ["", "0", "true", "yes"] {
            assert!(!on(Some(other)), "{other}");
            assert_eq!(full_damage(Some(other)), FullDamage::Keep, "{other}");
        }
        assert_eq!(full_damage(Some("1")), FullDamage::Discard);
        assert!(matches!(
            full_damage(Some("clear")),
            FullDamage::Clear(color) if color.r() == 1.0 && color.g() == 0.0 && color.b() == 1.0
        ));
    }
}
