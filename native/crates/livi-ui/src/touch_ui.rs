use std::cell::{Cell, RefCell};
use std::time::Duration;

use livi_core_client::Client;
use livi_core_proto::input::{Input, Point};
use livi_core_proto::state::Screen;
use livi_ui_model::touch::{Change, Fingers};
use slint::ComponentHandle;
use slint::winit_030::winit;

use crate::{AppWindow, Page, PhoneState};

const MOVE_EVERY: Duration = Duration::from_millis(16);

thread_local! {
    static FINGERS: RefCell<Fingers> = RefCell::new(Fingers::default());
    static FLUSH_DUE: Cell<bool> = const { Cell::new(false) };
}

thread_local! {
    // a plugin's own place on the phone's picture: x, y, width, height in pixels of the window
    static KEPT: Cell<Option<[f64; 4]>> = const { Cell::new(None) };
}

/// A plugin's button on the phone's picture: touches in this area stay with the plugin
pub fn keep(area: Option<[f64; 4]>) {
    KEPT.set(area);
}

fn kept(area: Option<[f64; 4]>, x: f64, y: f64) -> bool {
    area.is_some_and(|[ax, ay, w, h]| (ax..=ax + w).contains(&x) && (ay..=ay + h).contains(&y))
}

fn picture(ui: &AppWindow, x: f64, y: f64) -> Option<(f64, f64)> {
    let size = ui.window().size();
    let (ww, wh) = (f64::from(size.width), f64::from(size.height));
    let (sw, sh) = (
        f64::from(ui.get_stream_width().max(1)),
        f64::from(ui.get_stream_height().max(1)),
    );
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
    if kept(KEPT.get(), x, y) {
        return false;
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
        screen: Screen::Main,
        points,
    });
}

pub fn handle(ui: &AppWindow, client: &Client, touch: &winit::event::Touch) -> bool {
    let held = FINGERS.with_borrow(|f| f.holds(touch.id));
    let on_picture = ui.get_page() == Page::Projection
        && ui.get_phone() == PhoneState::Streaming
        && (held || for_phone(ui, touch.location.x, touch.location.y));
    if !on_picture && !held {
        return false;
    }
    let change = match touch.phase {
        winit::event::TouchPhase::Started => Change::Down,
        winit::event::TouchPhase::Moved => Change::Move,
        winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => Change::Up,
    };
    let change = if on_picture { change } else { Change::Up };
    let at = picture(ui, touch.location.x, touch.location.y);
    if let Some(points) = FINGERS.with_borrow_mut(|f| f.touch(touch.id, at, change)) {
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
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugins_area_keeps_its_touches() {
        let area = Some([10.0, 20.0, 80.0, 80.0]);
        assert!(kept(area, 10.0, 20.0));
        assert!(kept(area, 90.0, 100.0));
        assert!(!kept(area, 91.0, 50.0));
        assert!(!kept(None, 50.0, 50.0));
    }
}
