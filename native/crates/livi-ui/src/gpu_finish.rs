//! LIVI_UI_FINISH=1: after each frame a window waits with glFinish until the GPU has drawn it, before Slint swaps it.
//! Some drivers pass the buffer to the compositor without a fence for drawing still under way (PowerVR DDK 1.11 on the
//! GE8300, Wayland): the compositor then shows it half drawn or the one before, and the picture flickers between two
//! buffers. Measured on that board: with the wait the buffers no longer alternate, at the same frame rate. Off by
//! default: elsewhere it only makes the CPU wait.
//!
//! From the window's rendering notifier: Slint's Skia renderer calls AfterRendering once Skia has flushed the frame to
//! the driver and before the swap, with the frame's context current.

use std::sync::{LazyLock, OnceLock};

static WANTED: LazyLock<bool> =
    LazyLock::new(|| std::env::var("LIVI_UI_FINISH").is_ok_and(|v| v == "1"));

/// The driver's glFinish, 0 when it has none; the same in every context (EGL's proc addresses are)
static FINISH: OnceLock<usize> = OnceLock::new();

/// In AfterRendering: waits until the GPU has drawn the frame, if LIVI_UI_FINISH=1 and the window draws with OpenGL
pub fn wait(api: &slint::GraphicsAPI<'_>) {
    if !*WANTED {
        return;
    }
    let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api else {
        return;
    };
    let address = *FINISH.get_or_init(|| {
        let address = get_proc_address(c"glFinish") as usize;
        if address == 0 {
            eprintln!("[ui] the driver has no glFinish: LIVI_UI_FINISH does nothing");
        }
        address
    });
    if address != 0 {
        // SAFETY: the address is the driver's glFinish, which takes nothing and returns nothing; the frame's context is
        // current in the notifier
        let finish = unsafe { std::mem::transmute::<usize, unsafe extern "system" fn()>(address) };
        unsafe { finish() }
    }
}
