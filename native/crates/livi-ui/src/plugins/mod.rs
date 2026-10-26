//! UI plugins (docs/plugins.md): a folder with `plugin.json` and `.slint` files that adds pages, may replace the
//! navigation with its own bars and brings a start picture. The Slint interpreter compiles it at start (plugin.rs);
//! ComponentContainers in the window embed its components. Each of them gets the state in `livi` (state.rs,
//! plugin/api.slint) and reports the user's wishes through its `action` callback.

mod plugin;
mod state;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::Value as Json;
use slint::ComponentFactory;
use slint_interpreter::{ComponentDefinition, ComponentHandle, ComponentInstance, Value};

use crate::{AppWindow, Page, PhoneState, TabData};
use plugin::Plugin;

type ActionHandler = Rc<dyn Fn(&str, &str)>;

thread_local! {
    static PLUGIN: RefCell<Option<Rc<Plugin>>> = const { RefCell::new(None) };
    // components on screen, to update their state
    static LIVE: RefCell<Vec<slint::Weak<ComponentInstance>>> = const { RefCell::new(Vec::new()) };
    static STATE: RefCell<Option<Value>> = const { RefCell::new(None) };
    static ACTION: RefCell<Option<ActionHandler>> = const { RefCell::new(None) };
    static WARNED: Cell<bool> = const { Cell::new(false) };
}

/// Folder of the plugin: `LIVI_UI_PLUGIN`, otherwise `ui-plugin` in LIVI's user data folder if it holds a plugin.json.
fn folder() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("LIVI_UI_PLUGIN").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let dir = user_data()?.join("ui-plugin");
    dir.join("plugin.json").is_file().then_some(dir)
}

#[cfg(target_os = "macos")]
fn user_data() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/LIVI"))
}

#[cfg(not(target_os = "macos"))]
fn user_data() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(base.join("LIVI"))
}

/// `LIVI --check-plugin <folder>`: compiles a plugin without starting LIVI and reports what is wrong with it.
pub fn check(dir: &Path) -> bool {
    match plugin::compile(dir) {
        Ok(p) => {
            println!("[ui] plugin {} is fine ({} ms)", p.name, p.compiled_ms);
            true
        }
        Err(e) => {
            eprintln!("[ui] plugin {}: {e}", dir.display());
            false
        }
    }
}

/// Reads and compiles the plugin; without one, or if it does not compile, LIVI runs as it is.
pub fn load() {
    let Some(dir) = folder() else { return };
    match plugin::compile(&dir) {
        Ok(p) => PLUGIN.with_borrow_mut(|slot| *slot = Some(Rc::new(p))),
        Err(e) => eprintln!("[ui] plugin {}: {e}", dir.display()),
    }
}

fn plugin() -> Option<Rc<Plugin>> {
    PLUGIN.with_borrow(Clone::clone)
}

/// Path of the page the plugin wants at start, instead of LIVI's start page.
pub fn start_path() -> Option<String> {
    plugin()?.start.as_ref().map(|id| format!("/plugin/{id}"))
}

pub fn has_page(id: &str) -> bool {
    plugin().is_some_and(|p| p.page(id).is_some())
}

/// Tabs for the plugin's pages, shown before LIVI's own.
pub fn tabs() -> Vec<TabData> {
    plugin()
        .map(|p| {
            p.pages
                .iter()
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

fn factory(component: ComponentDefinition) -> ComponentFactory {
    ComponentFactory::new(move |ctx| {
        let instance = component
            .create_embedded(ctx)
            .map_err(|e| eprintln!("[ui] plugin component {}: {e}", component.name()))
            .ok()?;
        if let Some(state) = STATE.with_borrow(Clone::clone) {
            let _ = instance.set_property("livi", state);
        }
        let _ = instance.set_callback("action", |args| {
            let arg = |i: usize| match args.get(i) {
                Some(Value::String(s)) => s.to_string(),
                _ => String::new(),
            };
            let (verb, argument) = (arg(0), arg(1));
            // after the event: the action may replace the component that sent it
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(handle) = ACTION.with_borrow(Clone::clone) {
                    handle(&verb, &argument);
                }
            });
            Value::Void
        });
        LIVE.with_borrow_mut(|live| live.push(instance.as_weak()));
        crate::debug::trace(&format!("plugin component {} created", component.name()));
        Some(instance)
    })
}

/// Puts the plugin's start picture and bars into the window and keeps the handler for its actions.
pub fn install(ui: &AppWindow, on_action: impl Fn(&str, &str) + 'static) {
    let Some(p) = plugin() else { return };
    ACTION.with_borrow_mut(|a| *a = Some(Rc::new(on_action)));
    if let Some(splash) = &p.splash {
        ui.set_intro_image(splash.clone());
    }
    for (slot, component) in &p.shell {
        let f = factory(component.clone());
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
            _ => {
                ui.set_plugin_bottom(f);
                ui.set_has_plugin_bottom(true);
            }
        }
    }
    println!("[ui] plugin {} installed", p.name);
}

/// Shows the plugin page `ui.plugin-page-id` if the window is on Page::Plugin, and hides the bars on the pages the
/// plugin wants full screen.
pub fn show_page(ui: &AppWindow) {
    let Some(p) = plugin() else { return };
    ui.set_plugin_bars_hidden(p.fullscreen.contains(&current_path(ui)));
    if ui.get_page() != Page::Plugin {
        return;
    }
    if let Some(page) = p.page(&ui.get_plugin_page_id()) {
        ui.set_plugin_page(factory(page.component.clone()));
    }
}

/// Builds the state for the plugin and hands it to all its components on screen.
pub fn sync(ui: &AppWindow, core_state: &Json) {
    let Some(p) = plugin() else { return };
    let value = state::build(ui, &p, core_state, &current_path(ui), phone_state(ui));
    // unchanged state: no update, so nothing redraws (LIVI calls this every second for its clock); components created
    // later get the state in their factory
    let unchanged = STATE.with_borrow_mut(|st| {
        let same = st.as_ref().is_some_and(|st| state::same(st, &value));
        *st = Some(value.clone());
        same
    });
    if unchanged {
        return;
    }
    LIVE.with_borrow_mut(|live| {
        live.retain(|w| match w.upgrade() {
            Some(instance) => {
                if let Err(e) = instance.set_property("livi", value.clone())
                    && !WARNED.replace(true)
                {
                    eprintln!("[ui] plugin component without `in property <LiviState> livi`: {e}");
                }
                true
            }
            None => false,
        })
    });
}
