//! LIVI_COMPOSITOR_DMABUF_WAIT (see `levers`): a commit with a client's dma-buf applies only once the buffer's implicit
//! write fences have signalled (Smithay's `Dmabuf::generate_blocker`, as its example compositor does), so that the
//! compositor never samples a buffer the client's GPU is still drawing. A driver that puts no fence on its buffers changes
//! nothing: the buffer is ready at once (`AlreadyReady`) and the commit applies as before, so video and camera planes
//! run as without the switch. A fence that has not signalled within `HOLD_LIMIT` lets the commit go anyway, with a line
//! in the log, so that a stuck client never stops its window.
//!
//! With `early` the frame callbacks of a held commit go out at once, not after the frame that shows it: the client may
//! draw its next frame while the GPU still works on this one. That only overlaps CPU and GPU if the client has a buffer
//! free for it (three instead of two).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, RegistrationToken};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::{
    add_blocker, get_parent, with_states, Blocker, BlockerState, BufferAssignment, CompositorHandler,
    SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;

use crate::levers::DmabufWait;
use crate::state::LiviState;

/// How long a commit waits at most for the fences of its buffer. A whole frame of the UI takes a small embedded GPU up
/// to about 65 ms; far above that the fence is stuck.
pub const HOLD_LIMIT: Duration = Duration::from_millis(500);

/// A summary line after this many held commits (about a minute of UI frames)
const REPORT_EVERY: u32 = 1800;

/// The blocker of a held commit: released when the buffer's fences have signalled or the limit has passed.
struct Held<B> {
    fences: B,
    expired: Arc<AtomicBool>,
}

impl<B: Blocker> Blocker for Held<B> {
    fn state(&self) -> BlockerState {
        if self.expired.load(Ordering::SeqCst) {
            BlockerState::Released
        } else {
            self.fences.state()
        }
    }
}

/// Counts for the log: how many commits waited, how long, how many ran into the limit.
#[derive(Debug, Default)]
struct HoldStats {
    held: u32,
    over_limit: u32,
    total: Duration,
    longest: Duration,
}

impl HoldStats {
    /// One commit released; Some(line) when a summary is due
    fn count(&mut self, waited: Duration, over_limit: bool) -> Option<String> {
        self.held += 1;
        self.total += waited;
        self.longest = self.longest.max(waited);
        if over_limit {
            self.over_limit += 1;
        }
        (self.held % REPORT_EVERY == 0).then(|| self.line())
    }

    fn line(&self) -> String {
        let mean = self.total.as_secs_f64() * 1000.0 / f64::from(self.held.max(1));
        format!(
            "dma-buf wait: {} commits held, mean {mean:.1} ms, longest {:.1} ms, {} over the limit",
            self.held,
            self.longest.as_secs_f64() * 1000.0,
            self.over_limit
        )
    }
}

thread_local! {
    /// the compositor's event loop is one thread
    static HOLDS: RefCell<HoldStats> = RefCell::default();
}

fn count_hold(waited: Duration, over_limit: bool) {
    if let Some(line) = HOLDS.with_borrow_mut(|stats| stats.count(waited, over_limit)) {
        log::info!("{line}");
    }
}

/// From the pre-commit hook, when the commit brought no explicit sync point: hold it until its dma-buf is drawn.
pub fn hold_until_drawn(state: &mut LiviState, surface: &WlSurface, mode: DmabufWait) {
    let dmabuf = with_states(surface, |data| {
        match data.cached_state.get::<SurfaceAttributes>().pending().buffer.as_ref() {
            Some(BufferAssignment::NewBuffer(buffer)) => get_dmabuf(buffer).ok().cloned(),
            _ => None,
        }
    });
    let (Some(dmabuf), Some(client)) = (dmabuf, surface.client()) else {
        return;
    };
    // nothing to wait for (no fence, or all signalled): the commit applies as before
    let Ok((fences, source)) = dmabuf.generate_blocker(Interest::READ) else {
        return;
    };
    let since = Instant::now();
    let expired = Arc::new(AtomicBool::new(false));
    let limit_token: Rc<Cell<Option<RegistrationToken>>> = Rc::default();
    let fence_token: Rc<Cell<Option<RegistrationToken>>> = Rc::default();

    let signalled = {
        let (client, limit_token, fence_token) = (client.clone(), limit_token.clone(), fence_token.clone());
        state.loop_handle.insert_source(source, move |_, _, state| {
            fence_token.take();
            if let Some(token) = limit_token.take() {
                state.loop_handle.remove(token);
            }
            count_hold(since.elapsed(), false);
            let dh = state.display_handle.clone();
            state.client_compositor_state(&client).blocker_cleared(state, &dh);
            Ok(())
        })
    };
    let Ok(signalled) = signalled else {
        return;
    };
    fence_token.set(Some(signalled));
    let limit = {
        let (expired, fence_token, id) = (expired.clone(), fence_token.clone(), surface.id());
        state.loop_handle.insert_source(Timer::from_duration(HOLD_LIMIT), move |_, _, state| {
            limit_token_cleanup(&fence_token, state);
            expired.store(true, Ordering::SeqCst);
            let over = HOLDS.with_borrow(|stats| stats.over_limit);
            if over < 5 || over % 100 == 0 {
                log::warn!(
                    "dma-buf wait: the fence of {id} did not signal within {} ms, the commit goes anyway",
                    HOLD_LIMIT.as_millis()
                );
            }
            count_hold(since.elapsed(), true);
            let dh = state.display_handle.clone();
            state.client_compositor_state(&client).blocker_cleared(state, &dh);
            TimeoutAction::Drop
        })
    };
    if let Ok(token) = limit {
        limit_token.set(Some(token));
    }
    add_blocker(surface, Held { fences, expired });
    if mode == DmabufWait::Early {
        send_frames_now(state, surface);
    }
}

/// The limit has passed: the fence's source goes, it would only clear the blocker a second time
fn limit_token_cleanup(fence_token: &Rc<Cell<Option<RegistrationToken>>>, state: &mut LiviState) {
    if let Some(token) = fence_token.take() {
        state.loop_handle.remove(token);
    }
}

/// `early`: the frame callbacks of the held commit go out now, for a window on screen (as `host::send_frame_callbacks`)
fn send_frames_now(state: &LiviState, surface: &WlSurface) {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    let shown = state
        .toplevels
        .iter()
        .any(|t| t.toplevel.wl_surface() == &root && crate::host::wants_frames(t.embed, t.visible));
    if !shown {
        return;
    }
    let time_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32;
    with_states(surface, |data| {
        for cb in data.cached_state.get::<SurfaceAttributes>().pending().frame_callbacks.drain(..) {
            cb.done(time_ms);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fences(BlockerState);

    impl Blocker for Fences {
        fn state(&self) -> BlockerState {
            self.0
        }
    }

    #[test]
    fn a_held_commit_goes_when_its_fences_signal() {
        let expired = Arc::new(AtomicBool::new(false));
        assert_eq!(
            Held { fences: Fences(BlockerState::Pending), expired: expired.clone() }.state(),
            BlockerState::Pending
        );
        assert_eq!(Held { fences: Fences(BlockerState::Released), expired }.state(), BlockerState::Released);
    }

    #[test]
    fn a_held_commit_goes_after_the_limit_even_if_the_fence_is_stuck() {
        let expired = Arc::new(AtomicBool::new(false));
        let held = Held { fences: Fences(BlockerState::Pending), expired: expired.clone() };
        expired.store(true, Ordering::SeqCst);
        assert_eq!(held.state(), BlockerState::Released);
    }

    #[test]
    fn the_summary_counts_waits_and_the_limit() {
        let mut stats = HoldStats::default();
        for i in 1..REPORT_EVERY {
            assert!(stats.count(Duration::from_millis(10), i == 7).is_none());
        }
        let line = stats.count(Duration::from_millis(40), false).expect("a line at REPORT_EVERY");
        assert!(line.contains("1800 commits held"), "{line}");
        assert!(line.contains("longest 40.0 ms"), "{line}");
        assert!(line.contains("1 over the limit"), "{line}");
        assert!(line.contains("mean 10.0 ms"), "{line}");
    }
}
