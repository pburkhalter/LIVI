//! UI plugins (docs/plugins.md): a folder with `plugin.json` and `.slint` files that adds pages and may replace the
//! navigation with its own bars. The Slint interpreter compiles it at start; ComponentContainers in the window embed
//! its components. Each of them gets the state in `livi` (plugin/api.slint) and reports the user's wishes through its
//! `action` callback.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use serde_json::Value as Json;
use slint::{ComponentFactory, Image, Model, ModelRc, SharedString, VecModel};
use slint_interpreter::{ComponentDefinition, ComponentHandle, ComponentInstance, Compiler, Struct, Value};

use crate::{AppWindow, Page, PhoneState, TabData, Theme};

pub const API: i32 = 1;
const API_SOURCE: &str = include_str!("../plugin/api.slint");
// Virtual folder for "@livi/api.slint"; the file loader answers for it
const API_DIR: &str = "/livi-plugin-api";

struct PluginPage {
    id: String,
    title: String,
    icon: Image,
    component: ComponentDefinition,
}

struct Plugin {
    name: String,
    start: Option<String>,
    splash: Option<Image>,
    // pages shown without the bars, e.g. "/camera"
    fullscreen: Vec<String>,
    shell: HashMap<&'static str, ComponentDefinition>,
    pages: Vec<PluginPage>,
}

type ActionHandler = Rc<dyn Fn(&str, &str)>;

thread_local! {
    static PLUGIN: RefCell<Option<Rc<Plugin>>> = const { RefCell::new(None) };
    static LIVE: RefCell<Vec<slint::Weak<ComponentInstance>>> = const { RefCell::new(Vec::new()) };
    static STATE: RefCell<Option<Value>> = const { RefCell::new(None) };
    static ACTION: RefCell<Option<ActionHandler>> = const { RefCell::new(None) };
    static SHOWN: Cell<bool> = const { Cell::new(false) };
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

/// The compiler's futures only wait for our own file loader, which answers at once: polling them is enough.
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

fn text(v: &Json, key: &str) -> Option<String> {
    v[key].as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// `LIVI --check-plugin <folder>`: compiles a plugin without starting LIVI and reports what is wrong with it.
pub fn check(dir: &Path) -> bool {
    match compile(dir) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("[ui] plugin {}: {e}", dir.display());
            false
        }
    }
}

/// Reads and compiles the plugin; without one, or if it does not compile, LIVI runs as it is.
pub fn load() {
    let Some(dir) = folder() else { return };
    match compile(&dir) {
        Ok(plugin) => PLUGIN.with_borrow_mut(|p| *p = Some(Rc::new(plugin))),
        Err(e) => eprintln!("[ui] plugin {}: {e}", dir.display()),
    }
}

fn compile(dir: &Path) -> Result<Plugin, String> {
    let started = Instant::now();
    let manifest: Json = serde_json::from_str(
        &std::fs::read_to_string(dir.join("plugin.json")).map_err(|e| format!("plugin.json: {e}"))?,
    )
    .map_err(|e| format!("plugin.json: {e}"))?;
    let api = manifest["api"].as_i64().unwrap_or(0);
    if api != i64::from(API) {
        return Err(format!("written for plugin API {api}, LIVI has {API}"));
    }
    let file = text(&manifest, "file").ok_or("plugin.json names no \"file\"")?;
    let name = text(&manifest, "name").unwrap_or_else(|| file.clone());

    let mut compiler = Compiler::default();
    compiler.set_library_paths(HashMap::from([("livi".to_string(), PathBuf::from(API_DIR))]));
    compiler.set_file_loader(|path: &Path| {
        let ours = path == Path::new(API_DIR).join("api.slint");
        Box::pin(async move { ours.then(|| Ok(API_SOURCE.to_string())) })
    });
    let result = block_on(compiler.build_from_path(dir.join(&file)));
    for d in result.diagnostics() {
        eprintln!("[ui] plugin {name}: {d}");
    }
    if result.has_errors() {
        return Err("does not compile".into());
    }
    let component = |name: &str| {
        result
            .component(name)
            .ok_or_else(|| format!("{file} exports no component {name}"))
    };

    let mut shell = HashMap::new();
    for slot in ["background", "start", "end", "top", "bottom"] {
        if let Some(c) = manifest["shell"][slot].as_str() {
            shell.insert(slot, component(c)?);
        }
    }
    let mut pages = Vec::new();
    for p in manifest["pages"].as_array().into_iter().flatten() {
        let id = text(p, "id").ok_or("a page without \"id\"")?;
        let component = component(&text(p, "component").ok_or(format!("page {id} names no \"component\""))?)?;
        let icon = text(p, "icon")
            .and_then(|i| Image::load_from_path(&dir.join(i)).ok())
            .unwrap_or_default();
        pages.push(PluginPage {
            title: text(p, "title").unwrap_or_else(|| id.clone()),
            id,
            icon,
            component,
        });
    }
    let start = text(&manifest, "start").filter(|s| pages.iter().any(|p| p.id == *s));
    let splash = text(&manifest, "splash").and_then(|f| Image::load_from_path(&dir.join(f)).ok());
    let fullscreen = manifest["fullscreen"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect();
    println!(
        "[ui] plugin {name}: {} pages, bars {:?}, compiled in {} ms",
        pages.len(),
        shell.keys().collect::<Vec<_>>(),
        started.elapsed().as_millis()
    );
    Ok(Plugin {
        name,
        start,
        splash,
        fullscreen,
        shell,
        pages,
    })
}

fn plugin() -> Option<Rc<Plugin>> {
    PLUGIN.with_borrow(Clone::clone)
}

/// Path of the page the plugin wants at start, instead of LIVI's start page.
pub fn start_path() -> Option<String> {
    plugin()?.start.as_ref().map(|id| format!("/plugin/{id}"))
}

pub fn has_page(id: &str) -> bool {
    plugin().is_some_and(|p| p.pages.iter().any(|page| page.id == id))
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
        Some(instance)
    })
}

/// Puts the plugin's bars into the window and keeps the handler for its actions.
pub fn install(ui: &AppWindow, on_action: impl Fn(&str, &str) + 'static) {
    let Some(p) = plugin() else { return };
    ACTION.with_borrow_mut(|a| *a = Some(Rc::new(on_action)));
    if let Some(splash) = &p.splash {
        ui.set_intro_image(splash.clone());
    }
    for (slot, component) in &p.shell {
        let f = factory(component.clone());
        match *slot {
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
            "background" => {
                ui.set_plugin_background(f);
                ui.set_has_plugin_background(true);
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
    if let Some(p) = plugin() {
        let path = current_path(ui);
        ui.set_plugin_bars_hidden(p.fullscreen.iter().any(|f| *f == path));
    }
    if ui.get_page() != Page::Plugin {
        SHOWN.set(false);
        return;
    }
    let id = ui.get_plugin_page_id().to_string();
    let Some(p) = plugin() else { return };
    let Some(page) = p.pages.iter().find(|page| page.id == id) else {
        return;
    };
    ui.set_plugin_page(factory(page.component.clone()));
    SHOWN.set(true);
}

pub fn page_path(ui: &AppWindow) -> String {
    format!("/plugin/{}", ui.get_plugin_page_id())
}

fn current_path(ui: &AppWindow) -> String {
    match ui.get_page() {
        Page::Plugin => page_path(ui),
        Page::Projection => "/projection".into(),
        Page::Telemetry => "/telemetry".into(),
        Page::Media => "/media".into(),
        Page::Camera => "/camera".into(),
        Page::Settings => "/settings".into(),
    }
}

fn num(v: &Json) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

fn s(text: impl AsRef<str>) -> Value {
    Value::String(SharedString::from(text.as_ref()))
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Struct(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Struct>())
}

fn model(items: Vec<Value>) -> Value {
    Value::Model(ModelRc::from(Rc::new(VecModel::from(items))))
}

fn local_time() -> libc::tm {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    tm
}

fn date(tm: &libc::tm) -> Value {
    record(vec![
        ("year", Value::Number(f64::from(tm.tm_year + 1900))),
        ("month", Value::Number(f64::from(tm.tm_mon + 1))),
        ("day", Value::Number(f64::from(tm.tm_mday))),
        ("weekday", Value::Number(f64::from(tm.tm_wday))),
    ])
}

fn pages(ui: &AppWindow, current: &str) -> Value {
    let tabs = ui.get_tabs();
    let mut out = Vec::new();
    let p = plugin();
    for tab in tabs.iter() {
        let (path, title) = match tab.page {
            Page::Plugin => {
                let id = tab.plugin.to_string();
                let title = p
                    .as_ref()
                    .and_then(|p| p.pages.iter().find(|page| page.id == id).map(|page| page.title.clone()))
                    .unwrap_or_else(|| id.clone());
                (format!("/plugin/{id}"), title)
            }
            Page::Projection => ("/projection".into(), "Projection".into()),
            Page::Telemetry => ("/telemetry".into(), "Dashboard".into()),
            Page::Media => ("/media".into(), "Media".into()),
            Page::Camera => ("/camera".into(), "Camera".into()),
            Page::Settings => ("/settings".into(), "Settings".into()),
        };
        out.push(record(vec![
            ("active", Value::Bool(path == current)),
            ("path", s(&path)),
            ("title", s(title)),
            ("icon", Value::Image(tab.icon.clone())),
        ]));
    }
    model(out)
}

fn media(ui: &AppWindow) -> Value {
    // LIVI's media page shows a dash for a missing text; plugins get an empty one
    let text = |t: SharedString| s(if t.chars().all(|c| matches!(c, '-' | '—' | '–')) { "" } else { t.as_str() });
    record(vec![
        ("title", text(ui.get_track_title())),
        ("artist", text(ui.get_artist())),
        ("album", text(ui.get_album())),
        ("app", text(ui.get_app())),
        ("artwork", Value::Image(ui.get_artwork())),
        ("has-artwork", Value::Bool(ui.get_has_artwork())),
        ("playing", Value::Bool(ui.get_playing())),
        ("progress", Value::Number(f64::from(ui.get_progress()))),
        ("elapsed-ms", Value::Number(f64::from(ui.get_elapsed_ms()))),
        ("total-ms", Value::Number(f64::from(ui.get_total_ms()))),
    ])
}

fn nav(state: &Json) -> Value {
    let n = livi_ui_model::dashboard::nav::nav(state);
    record(vec![
        ("active", Value::Bool(n.active)),
        ("icon", Value::Image(crate::icons::icon(n.icon).unwrap_or_default())),
        ("maneuver", s(n.maneuver)),
        ("distance", s(n.distance)),
        ("road", s(n.road)),
        ("time-left", s(n.time_left)),
        ("destination", s(n.destination)),
        ("destination-distance", s(n.destination_distance)),
        ("app", s(n.app)),
    ])
}

/// Telemetry keys that have a field in LiviCar or LiviClimate (or that are not values); all others go to `other`.
const KNOWN: &[&str] = &[
    "ts",
    "speedKph",
    "rpm",
    "gear",
    "ambientC",
    "fuelPct",
    "rangeKm",
    "odometerKm",
    "odometerTripKm",
    "reverse",
    "lights",
    "highBeam",
    "hazards",
    "parkingBrake",
    "nightMode",
    "climate",
    "path",
    "gps",
    "gnss",
    "can",
    "tirePressure",
    "notification",
];

fn car(t: &Json) -> Value {
    let f = |key: &str| num(&t[key]);
    let b = |key: &str| Value::Bool(t[key].as_bool().unwrap_or(false));
    let pair = |key: &str| (Value::Number(f(key).unwrap_or(0.0)), Value::Bool(f(key).is_some()));
    let (speed, has_speed) = pair("speedKph");
    let (rpm, has_rpm) = pair("rpm");
    let (ambient, has_ambient) = pair("ambientC");
    let (fuel, has_fuel) = pair("fuelPct");
    let (range, has_range) = pair("rangeKm");
    let gear = match &t["gear"] {
        Json::String(g) => g.clone(),
        Json::Number(n) => n.to_string(),
        _ => String::new(),
    };
    record(vec![
        ("known", Value::Bool(t.as_object().is_some_and(|o| !o.is_empty()))),
        ("speed-kph", speed),
        ("has-speed", has_speed),
        ("rpm", rpm),
        ("has-rpm", has_rpm),
        ("gear", s(gear)),
        ("ambient-c", ambient),
        ("has-ambient", has_ambient),
        ("fuel-pct", fuel),
        ("has-fuel", has_fuel),
        ("range-km", range),
        ("has-range", has_range),
        ("odometer-km", Value::Number(f("odometerKm").unwrap_or(0.0))),
        ("trip-km", Value::Number(f("odometerTripKm").unwrap_or(0.0))),
        ("reverse", b("reverse")),
        ("lights", b("lights")),
        ("high-beam", b("highBeam")),
        ("hazards", b("hazards")),
        ("parking-brake", b("parkingBrake")),
        ("night", b("nightMode")),
    ])
}

fn climate(c: &Json) -> Value {
    let b = |key: &str| Value::Bool(c[key].as_bool().unwrap_or(false));
    let n = |key: &str| Value::Number(num(&c[key]).unwrap_or(0.0));
    let has = |key: &str| Value::Bool(num(&c[key]).is_some());
    let source = c["source"].as_str().unwrap_or_default();
    record(vec![
        ("known", Value::Bool(source == "external")),
        ("source", s(source)),
        ("on", b("on")),
        ("auto", b("auto")),
        ("ac", b("ac")),
        ("max-ac", b("maxAc")),
        ("max-defrost", b("maxDefrost")),
        ("recirculation", b("recirculation")),
        ("synced", b("zonesSynced")),
        ("driver-c", n("driverTargetC")),
        ("has-driver", has("driverTargetC")),
        ("passenger-c", n("passengerTargetC")),
        ("has-passenger", has("passengerTargetC")),
        ("fan-level", n("fanLevel")),
        ("fan-auto", b("fanAuto")),
        ("front-window-heating", n("frontWindowHeating")),
        ("rear-window-heating", n("rearWindowHeating")),
        ("driver-seat-heating", n("driverSeatHeating")),
        ("passenger-seat-heating", n("passengerSeatHeating")),
    ])
}

fn other(t: &Json) -> Value {
    let mut out = Vec::new();
    for (key, value) in t.as_object().into_iter().flatten() {
        if KNOWN.contains(&key.as_str()) {
            continue;
        }
        let text = match value {
            Json::String(v) => v.clone(),
            Json::Null => continue,
            v => v.to_string(),
        };
        out.push(record(vec![("key", s(key)), ("value", s(text))]));
    }
    model(out)
}

/// Builds the state for the plugin and hands it to all its components on screen.
pub fn sync(ui: &AppWindow, state: &Json) {
    if plugin().is_none() {
        return;
    }
    let current = current_path(ui);
    let phone = match ui.get_phone() {
        PhoneState::None => "none",
        PhoneState::Connected => "connected",
        PhoneState::Streaming => "streaming",
    };
    let t = &state["telemetry"];
    let tm = local_time();
    let value = record(vec![
        ("api", Value::Number(f64::from(API))),
        ("page", s(&current)),
        ("clock", s(format!("{:02}:{:02}", tm.tm_hour, tm.tm_min))),
        ("date", date(&tm)),
        ("dark", Value::Bool(ui.global::<Theme>().get_dark())),
        ("phone", s(phone)),
        ("pages", pages(ui, &current)),
        ("media", media(ui)),
        ("nav", nav(state)),
        ("car", car(t)),
        ("climate", climate(&t["climate"])),
        ("other", other(t)),
    ]);
    // unchanged state: no update, so nothing redraws (LIVI calls this every second for its clock); components created
    // later get the state in their factory
    let unchanged = STATE.with_borrow_mut(|st| {
        let same = st.as_ref() == Some(&value);
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
