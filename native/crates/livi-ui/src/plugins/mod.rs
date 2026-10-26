//! UI plugins (docs/plugins.md): a folder with `plugin.json` and `.slint` files that adds pages, may replace the
//! navigation with its own bars and brings a start picture. The Slint interpreter compiles it at start (plugin.rs);
//! ComponentContainers in the window embed its components. They read the state from the global `Livi` (state.rs,
//! plugin/api.slint) and report the user's wishes through its callback `action`.

mod plugin;
mod state;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::Value as Json;
use slint::{ComponentFactory, ModelRc, VecModel};
use slint_interpreter::{ComponentDefinition, ComponentHandle, ComponentInstance, Value};

use crate::{AppWindow, Page, PhoneState, PluginView, TabData};
use plugin::Plugin;

type ActionHandler = Rc<dyn Fn(&str, &str)>;

/// The global in plugin/api.slint that holds the state and the callback `action`
const GLOBAL: &str = "Livi";

thread_local! {
    static PLUGIN: RefCell<Option<Rc<Plugin>>> = const { RefCell::new(None) };
    // components on screen, to update their state
    static LIVE: RefCell<Vec<slint::Weak<ComponentInstance>>> = const { RefCell::new(Vec::new()) };
    // the properties of the global Livi as last handed out
    static STATE: RefCell<Vec<(&'static str, Value)>> = const { RefCell::new(Vec::new()) };
    static ACTION: RefCell<Option<ActionHandler>> = const { RefCell::new(None) };
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
        STATE.with_borrow(|state| {
            for (name, value) in state {
                let _ = instance.set_global_property(GLOBAL, name, value.clone());
            }
        });
        let _ = instance.set_global_callback(GLOBAL, "action", |args| {
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
        let _ = instance.set_global_callback(GLOBAL, "find", |args| {
            match (args.first(), args.get(1)) {
                (Some(Value::Model(values)), Some(Value::String(key))) => state::find(values, key),
                _ => Value::String(Default::default()),
            }
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
    let views: Vec<PluginView> = p
        .pages
        .iter()
        .map(|page| PluginView {
            id: page.id.as_str().into(),
            factory: factory(page.component.clone()),
        })
        .collect();
    ui.set_plugin_pages(ModelRc::new(VecModel::from(views)));
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
    if changed.is_empty() {
        return;
    }
    LIVE.with_borrow_mut(|live| {
        live.retain(|w| match w.upgrade() {
            Some(instance) => {
                for (name, value) in &changed {
                    let _ = instance.set_global_property(GLOBAL, name, value.clone());
                }
                true
            }
            None => false,
        })
    });
}
