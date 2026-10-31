//! UI plugins (docs/plugins.md): a folder with `plugin.json` and `.slint` files that adds pages, may replace the
//! navigation with its own bars and brings a start picture. The Slint interpreter compiles it at start
//! (interpreted.rs), or a build has one built in (compiled.rs, feature compiled-plugin). Its components read the state
//! from the global `Livi` (state.rs, plugin/api.slint) and report the user's wishes through its callback `action`.

#[cfg(all(feature = "compiled-plugin", feature = "interpreter"))]
compile_error!("compiled-plugin replaces interpreter: build with --no-default-features");
#[cfg(not(any(feature = "compiled-plugin", feature = "interpreter")))]
compile_error!("LIVI needs one of the features interpreter and compiled-plugin");

#[cfg(feature = "compiled-plugin")]
#[path = "compiled.rs"]
mod backend;
#[cfg(not(feature = "compiled-plugin"))]
#[path = "interpreted.rs"]
mod backend;
mod plugin;
mod settings;
mod state;
#[cfg(feature = "compiled-plugin")]
mod value;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use livi_ui_model::focus::{self, Targets};
use serde_json::Value as Json;
use slint::{ModelRc, VecModel};

use crate::{AppWindow, Page, PhoneState, PluginView, TabData};
use backend::Value;
use plugin::Plugin;

type ActionHandler = Rc<dyn Fn(&str, &str)>;
type EmitHandler = Rc<dyn Fn(&str, &str, &str)>;

thread_local! {
    static PLUGIN: RefCell<Option<Rc<Plugin>>> = const { RefCell::new(None) };
    // the properties of the global Livi as last handed out
    static STATE: RefCell<Vec<(&'static str, Value)>> = const { RefCell::new(Vec::new()) };
    static ACTION: RefCell<Option<ActionHandler>> = const { RefCell::new(None) };
    static EMIT: RefCell<Option<EmitHandler>> = const { RefCell::new(None) };
    // what the components can take the focus on, for keys and a rotary knob (action focus-targets)
    static TARGETS: RefCell<Targets> = const { RefCell::new(Targets::new()) };
    // the target last activated or changed with a key (Livi.press): id, step, serial
    static PRESS: RefCell<(String, i32, i32)> = const { RefCell::new((String::new(), 0, 0)) };
    // the key last pressed that LIVI has no use for (Livi.key): code, repeat, serial
    static KEY: RefCell<(String, bool, i32)> = const { RefCell::new((String::new(), false, 0)) };
}

/// `LIVI --check-plugin <folder>`: compiles a plugin without starting LIVI and reports what is wrong with it.
pub fn check(dir: &Path) -> bool {
    backend::check(dir)
}

/// Gets the plugin ready; without one, or if it does not compile, LIVI runs as it is.
pub fn load() {
    if let Some(p) = backend::load() {
        PLUGIN.with_borrow_mut(|slot| *slot = Some(Rc::new(p)));
    }
}

fn plugin() -> Option<Rc<Plugin>> {
    PLUGIN.with_borrow(Clone::clone)
}

/// Whether a plugin is there: the keys LIVI has no use for go to it
pub fn installed() -> bool {
    PLUGIN.with_borrow(Option::is_some)
}

/// Path of a tab as the plugin knows it: "/plugin/<id>" for its pages, otherwise LIVI's route
pub fn tab_path(page: Page, plugin_id: &str) -> String {
    state::tab_path(page, plugin_id)
}

/// The plugin's components declare what they can take the focus on (`Livi.action("focus-targets", ...)`)
pub fn declare_targets(argument: &str) -> Result<(), String> {
    TARGETS.with_borrow_mut(|t| t.declare(argument))
}

/// The targets the plugin declared for a page, by its path
pub fn page_targets(path: &str) -> Vec<focus::Target> {
    TARGETS.with_borrow(|t| t.page(path).to_vec())
}

/// The targets of the plugin's side bar
pub fn nav_targets() -> Vec<focus::Target> {
    TARGETS.with_borrow(|t| t.nav().to_vec())
}

/// A target was activated (step 0) or changed by a step with a key: the plugin's components hear of it with the
/// next state (`Livi.press`)
pub fn press(id: &str, step: i32) {
    PRESS.with_borrow_mut(|(target, last, serial)| {
        *target = id.to_string();
        *last = step;
        *serial = serial.wrapping_add(1);
    });
}

/// A key LIVI has no use for was pressed: the plugin hears of it with the next state (`Livi.key`)
pub fn unused_key(code: &str, repeat: bool) {
    KEY.with_borrow_mut(|(last, repeated, serial)| {
        *last = code.to_string();
        *repeated = repeat;
        *serial = serial.wrapping_add(1);
    });
}

fn press_value(target: &str, step: i32, serial: i32) -> Value {
    state::record(vec![
        ("target", state::s(target)),
        ("step", Value::Number(f64::from(step))),
        ("serial", Value::Number(f64::from(serial))),
    ])
}

fn key_value(code: &str, repeat: bool, serial: i32) -> Value {
    state::record(vec![
        ("code", state::s(code)),
        ("repeat", Value::Bool(repeat)),
        ("serial", Value::Number(f64::from(serial))),
    ])
}

/// The properties `press` and `key` of the global Livi
fn key_fields() -> [(&'static str, Value); 2] {
    [
        (
            "press",
            PRESS.with_borrow(|(t, step, n)| press_value(t, *step, *n)),
        ),
        (
            "key",
            KEY.with_borrow(|(code, repeat, n)| key_value(code, *repeat, *n)),
        ),
    ]
}

/// Path of the page the plugin wants at start, instead of LIVI's start page.
pub fn start_path() -> Option<String> {
    plugin()?.start.as_ref().map(|id| format!("/plugin/{id}"))
}

pub fn has_page(id: &str) -> bool {
    plugin().is_some_and(|p| p.page(id).is_some())
}

/// Name of the plugin and how long it took to compile, for the debug overlay
pub fn summary() -> Option<String> {
    plugin().map(|p| format!("{} {} ms", p.name, p.compiled_ms))
}

/// Tabs for the plugin's pages, shown before LIVI's own.
pub fn tabs() -> Vec<TabData> {
    plugin()
        .map(|p| {
            p.pages
                .iter()
                .filter(|page| page.tab)
                .map(|page| TabData {
                    page: Page::Plugin,
                    icon: page.icon.clone(),
                    plugin: page.id.as_str().into(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the plugin replaces this page of LIVI's, so its tab goes away
pub fn hides(page: Page) -> bool {
    plugin().is_some_and(|p| p.hide.contains(&state::tab_path(page, "")))
}

/// Id of the plugin's page that shows LIVI's settings ("settings" in the manifest), if it has one
pub fn settings_page() -> Option<String> {
    plugin()?
        .pages
        .iter()
        .find(|p| !p.settings.is_empty())
        .map(|p| p.id.clone())
}

/// Whether the plugin's settings page is on screen
pub fn settings_shown(ui: &AppWindow) -> bool {
    ui.get_page() == Page::Plugin
        && settings_page().is_some_and(|id| ui.get_plugin_page_id() == id.as_str())
}

/// Whether the plugin's settings page draws the screen LIVI's settings are on
pub fn settings_drawable(ui: &AppWindow) -> bool {
    let Some(p) = plugin() else { return false };
    p.pages
        .iter()
        .find(|p| !p.settings.is_empty())
        .is_some_and(|page| settings::drawable(ui, &page.settings))
}

/// A wish of the plugin for LIVI's settings, `Livi.action("settings", ...)`; false if it is not one
pub fn settings_act(ui: &AppWindow, argument: &str) -> bool {
    settings::act(ui, argument)
}

pub fn phone_state(ui: &AppWindow) -> &'static str {
    match ui.get_phone() {
        PhoneState::None => "none",
        PhoneState::Connected => "connected",
        PhoneState::Streaming => "streaming",
    }
}

pub fn page_path(ui: &AppWindow) -> String {
    state::tab_path(Page::Plugin, &ui.get_plugin_page_id())
}

pub fn current_path(ui: &AppWindow) -> String {
    state::tab_path(ui.get_page(), &ui.get_plugin_page_id())
}

/// `Livi.action`, after the event: the action may replace the component that sent it
fn action(verb: String, argument: String) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(handle) = ACTION.with_borrow(Clone::clone) {
            handle(&verb, &argument);
        }
    });
}

/// `Livi.emit`
fn emit(topic: &str, name: &str, value: &str) {
    if let Some(handle) = EMIT.with_borrow(Clone::clone) {
        handle(topic, name, value);
    }
}

/// Puts the plugin's start picture and bars into the window and keeps the handlers for its actions and for the events
/// it sends to programs beside LIVI (`Livi.emit`: topic, name, value).
pub fn install(
    ui: &AppWindow,
    on_action: impl Fn(&str, &str) + 'static,
    on_emit: impl Fn(&str, &str, &str) + 'static,
) {
    let Some(p) = plugin() else { return };
    ACTION.with_borrow_mut(|a| *a = Some(Rc::new(on_action)));
    EMIT.with_borrow_mut(|e| *e = Some(Rc::new(on_emit)));
    backend::connect(ui);
    if let Some(splash) = &p.splash {
        ui.set_intro_image(splash.clone());
        ui.set_intro_done(false);
    }
    for (slot, component) in &p.shell {
        let f = backend::factory(component.clone());
        match *slot {
            "background" => {
                ui.set_plugin_background(f);
                ui.set_has_plugin_background(true);
            }
            "start" => {
                ui.set_plugin_start(f);
                ui.set_has_plugin_start(true);
            }
            "end" => {
                ui.set_plugin_end(f);
                ui.set_has_plugin_end(true);
            }
            "top" => {
                ui.set_plugin_top(f);
                ui.set_has_plugin_top(true);
            }
            "bottom" => {
                ui.set_plugin_bottom(f);
                ui.set_has_plugin_bottom(true);
            }
            "overlay" => {
                ui.set_plugin_overlay(f);
                ui.set_has_plugin_overlay(true);
            }
            _ => {}
        }
    }
    for (page, component) in &p.empty {
        let f = backend::factory(component.clone());
        match *page {
            "projection" => {
                ui.set_plugin_no_phone(f);
                ui.set_has_plugin_no_phone(true);
            }
            "camera" => {
                ui.set_plugin_no_camera(f);
                ui.set_has_plugin_no_camera(true);
            }
            _ => {}
        }
    }
    if let Some(component) = p.frame.get("camera") {
        ui.set_plugin_camera_frame(backend::factory(component.clone()));
        ui.set_has_plugin_camera_frame(true);
    }
    if backend::EMBEDS {
        let views: Vec<PluginView> = p
            .pages
            .iter()
            .map(|page| PluginView {
                id: page.id.as_str().into(),
                factory: backend::factory(page.component.clone()),
            })
            .collect();
        ui.set_plugin_pages(ModelRc::new(VecModel::from(views)));
    }
    println!("[ui] plugin {} installed", p.name);
}

/// Hides the bars on the pages the plugin wants full screen; the window shows the plugin page `plugin-page-id` itself.
pub fn show_page(ui: &AppWindow) {
    let Some(p) = plugin() else { return };
    ui.set_plugin_bars_hidden(p.fullscreen.contains(&current_path(ui)));
}

/// Builds the state for the plugin and hands what changed to all its components on screen.
pub fn sync(ui: &AppWindow, core_state: &Json) {
    let Some(p) = plugin() else { return };
    let fields = state::build(ui, &p, core_state, &current_path(ui), phone_state(ui));
    // Only what changed: every binding that reads a property runs again when it is set, and LIVI calls this every
    // second for its clock. Components created later get all of it in their factory.
    let changed: Vec<(&'static str, Value)> = STATE.with_borrow_mut(|last| {
        let changed = fields
            .iter()
            .filter(|(name, value)| !last.iter().any(|(n, v)| n == name && state::same(v, value)))
            .cloned()
            .collect();
        *last = fields;
        changed
    });
    if !changed.is_empty() {
        backend::publish(ui, &changed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_targets_are_found_by_page_and_for_the_bar() {
        declare_targets("/plugin/home a b:slider").unwrap();
        declare_targets("nav home:current media").unwrap();
        assert!(declare_targets("home x").is_err());
        let page = page_targets("/plugin/home");
        assert_eq!(page.len(), 2);
        assert!(page[1].slider);
        assert!(page_targets("/plugin/media").is_empty());
        assert!(nav_targets()[0].current);
    }
}
