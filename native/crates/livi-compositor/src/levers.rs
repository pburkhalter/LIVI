//! Switches for the graphics chain, read once at the start. Each is off unless its variable is set; with none set the
//! compositor paces and draws as it does without them. They are here to be measured on the device.

use std::sync::LazyLock;

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

fn on(value: Option<&str>) -> bool {
    value == Some("1")
}

/// A line in the log when a switch is on: a measurement has to show what it ran with
pub fn announce() {
    let (early, hole, camera) = (*EARLY_CALLBACK, *UI_HOLE, *CAMERA_FIRST);
    if early || hole || camera {
        log::info!("graphics levers: early callback {early}, UI hole {hole}, camera first {camera}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_variable_every_lever_is_off() {
        assert!(!on(None));
    }

    #[test]
    fn only_the_documented_values_switch() {
        assert!(on(Some("1")));
        for other in ["", "0", "true", "yes"] {
            assert!(!on(Some(other)), "{other}");
        }
    }
}
