use std::cell::{Cell, RefCell};
use std::time::Instant;

use livi_core_client::Client;
use livi_core_proto::input::Input;
use livi_core_proto::message::Action;
use livi_ui_model::focus;
use livi_ui_model::keys::{
    Context, Focused, HELD_GAP, Outcome, Zone, bound_to, decide, held_factor, released,
};
use livi_ui_model::media::pressed;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{ComponentHandle, Model};

use crate::{
    AppWindow, DabState, Focus, FocusZone, Page, PhoneState, RowKind, ScreenKind, SettingsState,
};

const SEEK_STEP_MS: f32 = 15_000.0;
const LEVEL_STEP: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Back,
    Apply,
    Row(usize, RowKind),
    Tool(usize, usize),
    DialogButton(usize),
    Pick(usize),
    NumberMinus,
    Number,
    NumberPlus,
    Text,
    ColorHex,
    ColorReset,
    ColorSlider(i32),
    PageSlider,
    CalSlider(i32),
    Artwork,
    Prev,
    PlayPause,
    Next,
    Progress,
    Source(bool),
    Favorite,
    Station(usize),
    /// a target the UI plugin declared for the page on screen: its place in the list, whether it is a slider
    Plugin(usize, bool),
}

thread_local! {
    static TRAIL: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
    static HELD: Cell<Option<(Instant, Instant, f32)>> = const { Cell::new(None) };
    static KEY_REPEAT: Cell<bool> = const { Cell::new(false) };
    // a button of the plugin's side bar was pressed to open a page: the focus goes into the page once it is there
    static ENTER_NEXT_PAGE: Cell<bool> = const { Cell::new(false) };
}

fn settings_targets(s: &SettingsState) -> Vec<Target> {
    let mut out = Vec::new();
    let dialog = crate::settings_ui::dialog_buttons(s);
    if !dialog.is_empty() {
        return dialog.into_iter().map(Target::DialogButton).collect();
    }
    if s.get_can_back() {
        out.push(Target::Back);
    }
    if s.get_show_apply() {
        out.push(Target::Apply);
    }
    match s.get_kind() {
        ScreenKind::List => {
            let rows = s.get_rows();
            for (i, r) in rows.iter().enumerate() {
                match r.kind {
                    RowKind::Info | RowKind::Progress => {}
                    RowKind::Item => {
                        out.extend((0..r.tools.row_count()).map(|j| Target::Tool(i, j)))
                    }
                    RowKind::Device => {
                        if r.enabled {
                            out.push(Target::Row(i, RowKind::Device));
                        }
                        out.extend((0..r.tools.row_count()).map(|j| Target::Tool(i, j)))
                    }
                    kind => out.push(Target::Row(i, kind)),
                }
            }
        }
        ScreenKind::Select => out.extend((0..s.get_picks().row_count()).map(Target::Pick)),
        ScreenKind::Number => out.extend([Target::NumberMinus, Target::Number, Target::NumberPlus]),
        ScreenKind::Text => out.push(Target::Text),
        ScreenKind::Color => out.extend([
            Target::ColorHex,
            Target::ColorReset,
            Target::ColorSlider(0),
            Target::ColorSlider(1),
            Target::ColorSlider(2),
        ]),
        ScreenKind::Slider => out.push(Target::PageSlider),
        ScreenKind::Calibration => {
            out.extend((0..s.get_cal_knobs().row_count() as i32).map(Target::CalSlider))
        }
        ScreenKind::Pending => {}
    }
    out
}

/// What the plugin declared for the page on screen (docs/plugins.md, "Keys")
fn plugin_targets(ui: &AppWindow) -> Vec<focus::Target> {
    crate::plugins::page_targets(&crate::plugins::current_path(ui))
}

/// The targets of the plugin's side bar, where it takes the place of LIVI's navigation; empty otherwise, and then
/// the navigation goes over the tabs
fn plugin_nav(ui: &AppWindow) -> Vec<focus::Target> {
    if ui.get_has_plugin_start() || ui.get_has_plugin_end() {
        crate::plugins::nav_targets()
    } else {
        Vec::new()
    }
}

fn nav_len(ui: &AppWindow) -> usize {
    let bar = plugin_nav(ui);
    if bar.is_empty() {
        ui.get_tabs().row_count()
    } else {
        bar.len()
    }
}

/// How the plugin knows a target (`Livi.focus.target`): its own ids for what it declared, names for those of LIVI's
/// settings, which the plugin's settings page draws
fn target_id(t: Target, plugin: &[focus::Target]) -> String {
    match t {
        Target::Back => "back".into(),
        Target::Apply => "apply".into(),
        Target::Row(i, _) => format!("row:{i}"),
        Target::Tool(i, j) => format!("tool:{i}:{j}"),
        Target::DialogButton(i) => format!("dialog:{i}"),
        Target::Pick(i) => format!("pick:{i}"),
        Target::NumberMinus => "number-minus".into(),
        Target::Number => "number".into(),
        Target::NumberPlus => "number-plus".into(),
        Target::Text => "text".into(),
        Target::ColorHex => "color-hex".into(),
        Target::ColorReset => "color-reset".into(),
        Target::ColorSlider(w) => format!("color-slider:{w}"),
        Target::PageSlider => "level".into(),
        Target::CalSlider(i) => format!("knob:{i}"),
        Target::Artwork => "artwork".into(),
        Target::Prev => "prev".into(),
        Target::PlayPause => "play-pause".into(),
        Target::Next => "next".into(),
        Target::Progress => "progress".into(),
        Target::Source(false) => "source:phone".into(),
        Target::Source(true) => "source:dab".into(),
        Target::Favorite => "favorite".into(),
        Target::Station(i) => format!("station:{i}"),
        Target::Plugin(i, _) => plugin.get(i).map(|p| p.id.clone()).unwrap_or_default(),
    }
}

fn targets(ui: &AppWindow) -> Vec<Target> {
    // the plugin's settings page draws LIVI's settings: the targets of LIVI's own settings page
    if crate::plugins::settings_shown(ui) {
        return settings_targets(&ui.global::<SettingsState>());
    }
    match ui.get_page() {
        Page::Settings => settings_targets(&ui.global::<SettingsState>()),
        Page::Media => {
            let mut out = Vec::new();
            let d = ui.global::<DabState>();
            if d.get_receiver() {
                out.extend([Target::Source(false), Target::Source(true)]);
            }
            if d.get_receiver() && d.get_view_dab() {
                if d.get_art_shown() {
                    out.push(Target::Artwork);
                }
                out.extend([
                    Target::Prev,
                    Target::PlayPause,
                    Target::Next,
                    Target::Favorite,
                ]);
                if d.get_band_shown() {
                    out.extend((0..d.get_stations().row_count()).map(Target::Station));
                }
                return out;
            }
            if ui.global::<Focus>().get_media_art() {
                out.push(Target::Artwork);
            }
            out.extend([Target::Prev, Target::PlayPause, Target::Next]);
            if ui.get_can_seek() {
                out.push(Target::Progress);
            }
            out
        }
        // a page of the plugin, or one of LIVI's it draws on (a frame, an empty page): what it declared
        _ => plugin_targets(ui)
            .iter()
            .enumerate()
            .map(|(i, t)| Target::Plugin(i, t.slider))
            .collect(),
    }
}

fn kind(t: Target) -> Focused {
    match t {
        Target::Row(_, RowKind::Slider)
        | Target::ColorSlider(_)
        | Target::PageSlider
        | Target::CalSlider(_)
        | Target::Progress
        | Target::Plugin(_, true) => Focused::Slider,
        Target::Number => Focused::Number,
        Target::Text | Target::ColorHex => Focused::Text,
        _ => Focused::Plain,
    }
}

fn first_index(list: &[Target]) -> i32 {
    list.iter()
        .position(|t| !matches!(t, Target::Back | Target::Apply | Target::NumberMinus))
        .or(if list.is_empty() { None } else { Some(0) })
        .map_or(0, |i| i as i32)
}

fn zone(f: &Focus) -> Zone {
    match f.get_zone() {
        FocusZone::None => Zone::None,
        FocusZone::Nav => Zone::Nav,
        FocusZone::Main => Zone::Main,
    }
}

fn enter_main(ui: &AppWindow) {
    let f = ui.global::<Focus>();
    let list = targets(ui);
    if list.is_empty() {
        f.set_zone(FocusZone::None);
        return;
    }
    f.set_editing(false);
    f.set_main_index(first_index(&list));
    f.set_zone(FocusZone::Main);
}

fn to_nav(ui: &AppWindow) {
    let f = ui.global::<Focus>();
    let bar = plugin_nav(ui);
    // the plugin's bar: on the button the plugin marked as the page on screen, else where the focus was last
    let at = if bar.is_empty() {
        crate::active_tab(ui).unwrap_or(0)
    } else {
        let last = (f.get_nav_index().max(0) as usize).min(bar.len() - 1);
        bar.iter().position(|t| t.current).unwrap_or(last)
    };
    f.set_editing(false);
    f.set_nav_index(at as i32);
    f.set_zone(FocusZone::Nav);
}

/// Tells the plugin that a target of the page on screen was activated (step 0) or changed by a step
fn press_plugin(ui: &AppWindow, index: usize, step: i32) {
    if let Some(t) = plugin_targets(ui).get(index) {
        crate::plugins::press(&t.id, step);
    }
}

fn settle_after_settings_move(ui: &AppWindow, opened: bool, from: i32) {
    let f = ui.global::<Focus>();
    f.set_editing(false);
    let list = targets(ui);
    let index = if opened {
        TRAIL.with_borrow_mut(|t| t.push(from));
        first_index(&list)
    } else {
        TRAIL
            .with_borrow_mut(|t| t.pop())
            .unwrap_or_else(|| first_index(&list))
    };
    f.set_main_index(index.clamp(0, list.len().saturating_sub(1) as i32));
}

fn activate(ui: &AppWindow, target: Target) {
    let s = ui.global::<SettingsState>();
    let d = ui.global::<DabState>();
    let from = ui.global::<Focus>().get_main_index();
    match target {
        Target::Back => {
            s.invoke_back();
            settle_after_settings_move(ui, false, from);
        }
        Target::Apply => s.invoke_apply(),
        Target::Row(i, kind) => {
            let node = s.get_rows().row_data(i).map_or(0, |r| r.node);
            match kind {
                RowKind::Toggle => s.invoke_toggle(i as i32),
                RowKind::Button | RowKind::Device | RowKind::Pick | RowKind::Picture => {
                    let depth = crate::settings_ui::depth();
                    s.invoke_press(i as i32);
                    let now = crate::settings_ui::depth();
                    if now > depth {
                        settle_after_settings_move(ui, true, from);
                    } else if now < depth {
                        TRAIL.with_borrow_mut(|t| {
                            t.truncate(t.len().saturating_sub(depth - now - 1))
                        });
                        settle_after_settings_move(ui, false, from);
                    }
                }
                RowKind::KeyBinding => s.invoke_capture(i as i32),
                RowKind::Slider | RowKind::Devices => {}
                _ => {
                    let depth = crate::settings_ui::depth();
                    s.invoke_open(node);
                    if crate::settings_ui::depth() > depth {
                        settle_after_settings_move(ui, true, from);
                    }
                }
            }
        }
        Target::Tool(i, j) => s.invoke_tool(i as i32, j as i32),
        Target::DialogButton(i) => s.invoke_dialog_press(i as i32),
        Target::Pick(i) => {
            let depth = crate::settings_ui::depth();
            s.invoke_pick(i as i32);
            if crate::settings_ui::depth() < depth {
                settle_after_settings_move(ui, false, from);
            }
        }
        Target::ColorReset => s.invoke_color_reset(),
        Target::NumberMinus => adjust(ui, Target::Number, -1),
        Target::NumberPlus => adjust(ui, Target::Number, 1),
        Target::Artwork => {
            let on = !ui.get_show_spectrum();
            ui.set_show_spectrum(on);
            ui.invoke_spectrum_changed(on);
        }
        Target::Prev => ui.invoke_prev(),
        Target::PlayPause => ui.invoke_play_pause(),
        Target::Next => ui.invoke_next(),
        Target::Source(false) => d.invoke_phone(),
        Target::Source(true) => d.invoke_resume(),
        Target::Favorite => d.invoke_toggle_favorite(),
        Target::Station(i) => {
            if let Some(st) = d.get_stations().row_data(i) {
                d.invoke_pick(st.service_id);
            }
        }
        Target::Plugin(i, _) => press_plugin(ui, i, 0),
        _ => {}
    }
}

fn adjust(ui: &AppWindow, target: Target, delta: i32) {
    let s = ui.global::<SettingsState>();
    let d = delta as f32;
    match target {
        Target::Row(i, RowKind::Slider) => {
            if let Some(r) = s.get_rows().row_data(i) {
                s.invoke_slide_done(r.node, (r.level + d * LEVEL_STEP).clamp(0.0, 1.0));
            }
        }
        Target::PageSlider => s.invoke_level_done((s.get_level() + d * LEVEL_STEP).clamp(0.0, 1.0)),
        Target::CalSlider(i) => {
            if let Some(k) = s.get_cal_knobs().row_data(i as usize) {
                s.invoke_cal_done(i, (k.level + d * LEVEL_STEP).clamp(0.0, 1.0));
            }
        }
        Target::ColorSlider(w) => {
            let (now, max) = match w {
                0 => (s.get_color_hue(), 359.0),
                1 => (s.get_color_saturation(), 100.0),
                _ => (s.get_color_lightness(), 100.0),
            };
            s.invoke_color_slide_done(w, (now + d).clamp(0.0, max));
        }
        Target::Number => {
            let now = Instant::now();
            let (started, from) = match HELD.get() {
                Some((started, last, v)) if now - last < HELD_GAP => {
                    (if KEY_REPEAT.get() { started } else { now }, v)
                }
                _ => (now, s.get_number_value()),
            };
            let mut v = from + d * s.get_number_step() * held_factor(now - started);
            if s.get_number_has_min() {
                v = v.max(s.get_number_min());
            }
            if s.get_number_has_max() {
                v = v.min(s.get_number_max());
            }
            HELD.set(Some((started, now, v)));
            s.invoke_number_set(v);
        }
        Target::Progress => {
            let total = ui.get_total_ms();
            if total > 0.0 {
                let at = (ui.get_elapsed_ms() + d * SEEK_STEP_MS).clamp(0.0, total);
                ui.invoke_seek(at / total);
            }
        }
        Target::Plugin(i, true) => press_plugin(ui, i, delta),
        _ => {}
    }
}

fn open_tab(ui: &AppWindow, enter: bool) {
    let f = ui.global::<Focus>();
    let index = f.get_nav_index().max(0) as usize;
    let bar = plugin_nav(ui);
    if !bar.is_empty() {
        // a button of the plugin's bar: the plugin does what it does; a page it opens comes after this event, the
        // focus goes into it then (page_shown)
        if let Some(t) = bar.get(index) {
            crate::plugins::press(&t.id, 0);
            ENTER_NEXT_PAGE.set(enter);
        }
        return;
    }
    let Some(tab) = ui.get_tabs().row_data(index) else {
        return;
    };
    ui.set_plugin_page_id(tab.plugin.clone());
    ui.set_page(tab.page);
    ui.invoke_page_chosen(tab.page);
    TRAIL.with_borrow_mut(Vec::clear);
    if enter && tab.page != Page::Projection {
        enter_main(ui);
    }
}

fn apply(ui: &AppWindow, client: &Client, code: &str, outcome: Outcome) -> EventResult {
    let f = ui.global::<Focus>();
    let list = targets(ui);
    let current = list.get(f.get_main_index().max(0) as usize).copied();
    match outcome {
        Outcome::Pass => return EventResult::Propagate,
        Outcome::Ignore => {}
        Outcome::ToCore(code) => {
            let name = bound_to(&client.state()["config"], &code);
            if let Some(p) = name.as_deref().and_then(pressed) {
                crate::media_ui::pressed_feedback(ui, p);
            }
            client.input(Input::Key { code, down: true });
        }
        Outcome::CycleSession => {
            client.act(Action::NextDevice);
            let name = livi_ui_model::overlay::cycled(&client.state());
            if let Some(name) = name {
                crate::overlay_ui::phone(ui, &name);
            }
        }
        Outcome::NavStep(d) => {
            let n = nav_len(ui) as i32;
            if f.get_zone() != FocusZone::Nav {
                to_nav(ui);
            } else if n > 0 {
                f.set_nav_index((f.get_nav_index() + d).clamp(0, n - 1));
            }
        }
        Outcome::NavOpen { enter_main } => open_tab(ui, enter_main),
        Outcome::EnterMain => enter_main(ui),
        Outcome::ToNav => to_nav(ui),
        Outcome::MainStep(d) => {
            if !list.is_empty() {
                f.set_editing(false);
                f.set_main_index((f.get_main_index() + d).clamp(0, list.len() as i32 - 1));
            }
        }
        Outcome::Activate => {
            if let Some(t) = current {
                activate(ui, t);
            }
        }
        Outcome::StartEditing => f.set_editing(true),
        Outcome::StopEditing => {
            let enter = code == "Enter" || code == "NumpadEnter";
            if enter && current.is_some_and(|t| matches!(kind(t), Focused::Text | Focused::Number))
            {
                return EventResult::Propagate;
            }
            f.set_editing(false);
        }
        Outcome::Adjust(d) => {
            if let Some(t) = current {
                adjust(ui, t, d);
            }
        }
        Outcome::SettingsBack => {
            let from = f.get_main_index();
            ui.global::<SettingsState>().invoke_back();
            settle_after_settings_move(ui, false, from);
        }
        Outcome::Pager(by) => client.act(Action::StepClusterLayout { by }),
        Outcome::ToPlugin(code) => crate::plugins::unused_key(&code, KEY_REPEAT.get()),
    }
    // the plugin draws the focus (Livi.focus) and hears of presses and keys: now, not with the next second
    crate::sync_plugin(ui);
    EventResult::PreventDefault
}

fn context(ui: &AppWindow, repeat: bool) -> Context {
    let f = ui.global::<Focus>();
    let list = targets(ui);
    let mut z = zone(&f);
    if z == Zone::Main && list.is_empty() {
        z = Zone::None;
    }
    let focused = list
        .get(f.get_main_index().max(0) as usize)
        .copied()
        .map_or(Focused::Plain, kind);
    let page = ui.get_page();
    Context {
        zone: z,
        focused,
        editing: f.get_editing(),
        projection: page == Page::Projection,
        streaming: ui.get_phone() == PhoneState::Streaming,
        telemetry: page == Page::Telemetry,
        settings_depth: if page == Page::Settings || crate::plugins::settings_shown(ui) {
            crate::settings_ui::depth()
        } else {
            0
        },
        repeat,
        plugin: crate::plugins::installed(),
    }
}

pub fn wire(ui: &AppWindow, client: &Client) {
    let weak = ui.as_weak();
    let client = client.clone();
    ui.window().on_winit_window_event(move |_, event| {
        let Some(ui) = weak.upgrade() else {
            return EventResult::Propagate;
        };
        match event {
            winit::event::WindowEvent::KeyboardInput { event, .. } => {
                let winit::keyboard::PhysicalKey::Code(key) = event.physical_key else {
                    return EventResult::Propagate;
                };
                let code = format!("{key:?}");
                ui.invoke_wake_nav();
                if crate::settings_ui::capturing() {
                    if event.state == winit::event::ElementState::Pressed && !event.repeat {
                        crate::settings_ui::captured(&ui, &client, &code);
                    }
                    return EventResult::PreventDefault;
                }
                let state = client.state();
                let config = state["config"].clone();
                drop(state);
                if event.state == winit::event::ElementState::Released {
                    if let Some(code) = released(&code, &config) {
                        client.input(Input::Key { code, down: false });
                    }
                    return EventResult::Propagate;
                }
                KEY_REPEAT.set(event.repeat);
                ENTER_NEXT_PAGE.set(false);
                let ctx = context(&ui, event.repeat);
                let outcome = decide(&code, ctx, &config);
                apply(&ui, &client, &code, outcome)
            }
            winit::event::WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                crate::system_scale_changed(&ui, *scale_factor as f32);
                EventResult::Propagate
            }
            winit::event::WindowEvent::Resized(_) => {
                let later = weak.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(ui) = later.upgrade() {
                        crate::apply_zoom(&ui);
                    }
                });
                EventResult::Propagate
            }
            winit::event::WindowEvent::MouseInput {
                state: winit::event::ElementState::Pressed,
                ..
            }
            | winit::event::WindowEvent::Touch(_) => {
                let f = ui.global::<Focus>();
                ENTER_NEXT_PAGE.set(false);
                if !f.get_editing() && f.get_zone() != FocusZone::None {
                    f.set_zone(FocusZone::None);
                    crate::sync_plugin(&ui);
                }
                let later = weak.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(ui) = later.upgrade() {
                        ui.invoke_wake_nav();
                    }
                });
                if let winit::event::WindowEvent::Touch(touch) = event
                    && crate::touch_ui::handle(&ui, &client, touch)
                {
                    return EventResult::PreventDefault;
                }
                EventResult::Propagate
            }
            winit::event::WindowEvent::CursorMoved { position, .. } => {
                crate::cursor_ui::moved(&ui, *position);
                ui.invoke_wake_nav();
                EventResult::Propagate
            }
            winit::event::WindowEvent::MouseWheel { .. } => {
                ui.invoke_wake_nav();
                EventResult::Propagate
            }
            _ => EventResult::Propagate,
        }
    });
}

pub fn text_done(ui: &AppWindow) {
    ui.global::<Focus>().set_editing(false);
}

/// Where the focus is, for the plugin (`Livi.focus`): the zone ("", "nav" or "page"), the id of the target there
/// (in the navigation the path of the tab, unless the plugin's bar declared its own) and whether it is being edited
pub fn focused(ui: &AppWindow) -> (&'static str, String, bool) {
    let f = ui.global::<Focus>();
    match f.get_zone() {
        FocusZone::None => ("", String::new(), false),
        FocusZone::Nav => ("nav", nav_id(ui, f.get_nav_index().max(0) as usize), false),
        FocusZone::Main => match targets(ui).get(f.get_main_index().max(0) as usize) {
            Some(&t) => ("page", target_id(t, &plugin_targets(ui)), f.get_editing()),
            None => ("", String::new(), false),
        },
    }
}

/// The id of the navigation's target at `index`: the plugin bar's own, else the path of the tab
fn nav_id(ui: &AppWindow, index: usize) -> String {
    let bar = plugin_nav(ui);
    if !bar.is_empty() {
        return bar.get(index).map(|t| t.id.clone()).unwrap_or_default();
    }
    match ui.get_tabs().row_data(index) {
        Some(tab) => crate::plugins::tab_path(tab.page, &tab.plugin),
        None => String::new(),
    }
}

/// A page came on screen: if a button of the plugin's bar opened it with a key, the focus goes into it
pub fn page_shown(ui: &AppWindow) {
    if ENTER_NEXT_PAGE.take() && ui.get_page() != Page::Projection {
        TRAIL.with_borrow_mut(Vec::clear);
        enter_main(ui);
        crate::sync_plugin(ui);
    }
}

/// The plugin declared targets anew: the focus stays within what there is
pub fn targets_changed(ui: &AppWindow) {
    let f = ui.global::<Focus>();
    match f.get_zone() {
        FocusZone::Main => {
            let n = targets(ui).len() as i32;
            if n == 0 {
                f.set_zone(FocusZone::None);
            } else if f.get_main_index() >= n {
                f.set_main_index(n - 1);
            }
        }
        FocusZone::Nav => {
            let n = nav_len(ui) as i32;
            if n > 0 && f.get_nav_index() >= n {
                f.set_nav_index(n - 1);
            }
        }
        FocusZone::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_have_the_ids_the_plugin_api_names() {
        let declared = vec![focus::Target {
            id: "volume".into(),
            slider: true,
            current: false,
        }];
        assert_eq!(target_id(Target::Back, &declared), "back");
        assert_eq!(target_id(Target::Apply, &declared), "apply");
        assert_eq!(
            target_id(Target::Row(3, RowKind::Toggle), &declared),
            "row:3"
        );
        assert_eq!(target_id(Target::Tool(3, 1), &declared), "tool:3:1");
        assert_eq!(target_id(Target::DialogButton(0), &declared), "dialog:0");
        assert_eq!(target_id(Target::Pick(2), &declared), "pick:2");
        assert_eq!(target_id(Target::Number, &declared), "number");
        assert_eq!(
            target_id(Target::ColorSlider(1), &declared),
            "color-slider:1"
        );
        assert_eq!(target_id(Target::PageSlider, &declared), "level");
        assert_eq!(target_id(Target::CalSlider(1), &declared), "knob:1");
        assert_eq!(target_id(Target::Plugin(0, true), &declared), "volume");
        assert_eq!(target_id(Target::Plugin(5, false), &declared), "");
    }

    #[test]
    fn a_plugin_slider_is_a_slider() {
        assert_eq!(kind(Target::Plugin(0, true)), Focused::Slider);
        assert_eq!(kind(Target::Plugin(0, false)), Focused::Plain);
    }
}
