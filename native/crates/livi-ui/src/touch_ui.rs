use std::cell::{Cell, RefCell};
use std::time::Duration;

use livi_core_client::Client;
use livi_core_proto::input::{Input, Point};
use livi_core_proto::state::Screen;
use livi_ui_model::touch::{Change, Fingers};
use slint::ComponentHandle;
use slint::winit_030::winit;

use crate::{AppWindow, DashState, Page, PhoneState, TouchPhase};

const MOVE_EVERY: Duration = Duration::from_millis(16);
/// The finger the dashboard's pointer counts as: the mouse, or the first finger on the map (map_finger)
const POINTER: u64 = u64::MAX;
/// The key that presses the knob of the map
const KNOB_KEY: &str = "Space";
/// Pixels of a trackpad or a fine wheel for one detent of the knob
const WHEEL_DETENT: f64 = 40.0;

thread_local! {
    static FINGERS: RefCell<Fingers> = RefCell::new(Fingers::default());
    static FLUSH_DUE: Cell<bool> = const { Cell::new(false) };
    // the screen of the fingers held now
    static TARGET: Cell<Screen> = const { Cell::new(Screen::Main) };
    // the finger on the map that goes through Slint
    static FIRST: Cell<Option<u64>> = const { Cell::new(None) };
    // what the wheel turned short of a detent
    static WHEEL: Cell<f64> = const { Cell::new(0.0) };
}

fn picture(ui: &AppWindow, x: f64, y: f64) -> Option<(f64, f64)> {
    let stream = (
        f64::from(ui.get_stream_width().max(1)),
        f64::from(ui.get_stream_height().max(1)),
    );
    fitted(ui, stream, x, y)
}

/// (x, y), in pixels of the window, on the phone's map: it shows as large as its shape allows, like the main picture
fn map_picture(ui: &AppWindow, client: &Client, x: f64, y: f64) -> Option<(f64, f64)> {
    let state = client.state();
    let side = |key: &str| state["config"][key].as_f64().unwrap_or(0.0).max(1.0);
    let stream = (side("clusterWidth"), side("clusterHeight"));
    drop(state);
    fitted(ui, stream, x, y)
}

fn fitted(ui: &AppWindow, (sw, sh): (f64, f64), x: f64, y: f64) -> Option<(f64, f64)> {
    let size = ui.window().size();
    let (ww, wh) = (f64::from(size.width), f64::from(size.height));
    let pw = ww.min(wh * sw / sh);
    let ph = wh.min(ww * sh / sw);
    let (lx, ly) = (x - (ww - pw) / 2.0, y - (wh - ph) / 2.0);
    (pw > 0.0 && ph > 0.0 && (0.0..=pw).contains(&lx) && (0.0..=ph).contains(&ly))
        .then(|| (lx / pw, ly / ph))
}

/// Whether a touch starting at (x, y), in pixels of the screen, is the phone's: not on the button that switches the
/// picture between card and full screen, and in the card while it shows as one (app.slint)
fn for_phone(ui: &AppWindow, x: f64, y: f64) -> bool {
    let scale = f64::from(ui.window().scale_factor());
    let (x, y) = (x / scale, y / scale);
    let f = |v: f32| f64::from(v);
    if ui.get_video_button_shown() {
        let (bx, by, size) = (
            f(ui.get_video_button_x()),
            f(ui.get_video_button_y()),
            f(ui.get_video_button_size()),
        );
        if (bx..=bx + size).contains(&x) && (by..=by + size).contains(&y) {
            return false;
        }
    }
    if !ui.get_video_card_shown() {
        return true;
    }
    let size = ui.window().size().to_logical(ui.window().scale_factor());
    (f(ui.get_card_left())..=f(size.width) - f(ui.get_card_right())).contains(&x)
        && (f(ui.get_card_top())..=f(size.height) - f(ui.get_card_bottom())).contains(&y)
}

fn send(client: &Client, points: Vec<Point>) {
    client.input(Input::Pointer {
        screen: TARGET.get(),
        points,
    });
}

fn pass(client: &Client, screen: Screen, finger: u64, at: Option<(f64, f64)>, change: Change) {
    // all fingers of a touch go to the screen of the first
    if FINGERS.with_borrow(Fingers::is_empty) {
        TARGET.set(screen);
    }
    if let Some(points) = FINGERS.with_borrow_mut(|f| f.touch(finger, at, change)) {
        send(client, points);
    } else if change == Change::Move && !FLUSH_DUE.replace(true) {
        let client = client.clone();
        slint::Timer::single_shot(MOVE_EVERY, move || {
            FLUSH_DUE.set(false);
            if let Some(points) = FINGERS.with_borrow_mut(Fingers::flush) {
                send(&client, points);
            }
        });
    }
}

fn change_of(phase: winit::event::TouchPhase) -> Change {
    match phase {
        winit::event::TouchPhase::Started => Change::Down,
        winit::event::TouchPhase::Moved => Change::Move,
        winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => Change::Up,
    }
}

/// The phone's map shows on LIVI's page and takes input (trial LIVI_CLUSTER_TOUCH)
fn on_map(ui: &AppWindow) -> bool {
    ui.global::<DashState>().get_map_touch()
        && ui.get_page() == Page::Telemetry
        && crate::dashboard_ui::map_shown(ui)
}

/// The dashboard's pointer on the map, (x, y) in logical pixels of the window
pub fn map_pointer(ui: &AppWindow, client: &Client, phase: TouchPhase, x: f32, y: f32) {
    let change = match phase {
        TouchPhase::Down => Change::Down,
        TouchPhase::Move => Change::Move,
        TouchPhase::Up | TouchPhase::Cancel => Change::Up,
    };
    let scale = f64::from(ui.window().scale_factor());
    let at = map_picture(ui, client, f64::from(x) * scale, f64::from(y) * scale);
    pass(client, Screen::Cluster, POINTER, at, change);
}

/// The first finger goes on through Slint, like the mouse: on the dashboard it comes back as the pointer, on the bars
/// and dots above the map it stays theirs. The fingers after it join it here.
fn map_finger(ui: &AppWindow, client: &Client, touch: &winit::event::Touch) -> bool {
    let (pointer, held) = FINGERS.with_borrow(|f| (f.holds(POINTER), f.holds(touch.id)));
    if FIRST.get() == Some(touch.id) || !(pointer || held) {
        FIRST.set(Some(touch.id));
        return false;
    }
    let at = map_picture(ui, client, touch.location.x, touch.location.y);
    pass(
        client,
        Screen::Cluster,
        touch.id,
        at,
        change_of(touch.phase),
    );
    true
}

/// The map left the screen: a pointer held on it hears of no release any more
pub fn map_leaving(ui: &AppWindow, client: &Client) {
    if !on_map(ui) && FINGERS.with_borrow(|f| f.holds(POINTER)) {
        pass(client, Screen::Cluster, POINTER, None, Change::Up);
    }
}

/// The wheel turns the knob of the map, away from the user is to the right
pub fn map_wheel(ui: &AppWindow, client: &Client, delta: &winit::event::MouseScrollDelta) -> bool {
    if !on_map(ui) {
        return false;
    }
    let turned = match delta {
        winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
        winit::event::MouseScrollDelta::PixelDelta(p) => p.y / WHEEL_DETENT,
    };
    let total = WHEEL.get() + turned;
    WHEEL.set(total.fract());
    for _ in 0..total.abs().trunc() as u32 {
        client.input(Input::ClusterKnob {
            wheel: total.signum(),
            select: false,
        });
    }
    true
}

/// The key presses the knob of the map for as long as it is held
pub fn map_key(ui: &AppWindow, client: &Client, code: &str, down: bool, repeat: bool) -> bool {
    if code != KNOB_KEY || !on_map(ui) {
        return false;
    }
    if !repeat {
        client.input(Input::ClusterKnob {
            wheel: 0.0,
            select: down,
        });
    }
    true
}

pub fn handle(ui: &AppWindow, client: &Client, touch: &winit::event::Touch) -> bool {
    if on_map(ui) {
        return map_finger(ui, client, touch);
    }
    let held = FINGERS.with_borrow(|f| f.holds(touch.id));
    let on_picture = ui.get_page() == Page::Projection
        && ui.get_phone() == PhoneState::Streaming
        && (held || for_phone(ui, touch.location.x, touch.location.y));
    if !on_picture && !held {
        return false;
    }
    let change = if on_picture {
        change_of(touch.phase)
    } else {
        Change::Up
    };
    let at = picture(ui, touch.location.x, touch.location.y);
    pass(client, Screen::Main, touch.id, at, change);
    true
}
