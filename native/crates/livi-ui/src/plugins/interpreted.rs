//! A plugin in a folder, compiled by the Slint interpreter at start; ComponentContainers in the window embed its
//! components, each of which has the global `Livi` for itself.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use slint::{ComponentFactory, Image};
use slint_interpreter::{Compiler, ComponentDefinition, ComponentHandle, ComponentInstance};
pub use slint_interpreter::{Struct, Value};

use super::plugin::{self, Plugin};
use super::{KEY, PRESS, STATE, key_value, press_value, state};
use crate::AppWindow;

pub type Component = ComponentDefinition;

const API_SOURCE: &str = include_str!("../../plugin/api.slint");
// Virtual folder for "@livi/api.slint"; the file loader answers for it
const API_DIR: &str = "/livi-plugin-api";
/// The global in plugin/api.slint that holds the state and the callback `action`
const GLOBAL: &str = "Livi";

thread_local! {
    // components on screen, to update their state
    static LIVE: RefCell<Vec<slint::Weak<ComponentInstance>>> = const { RefCell::new(Vec::new()) };
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
    match compile(dir) {
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

/// Reads and compiles the plugin, if there is one; if it does not compile, LIVI runs as it is.
pub fn load() -> Option<Plugin> {
    let dir = folder()?;
    compile(&dir)
        .map_err(|e| eprintln!("[ui] plugin {}: {e}", dir.display()))
        .ok()
}

/// What a component created now gets of `press` and `key`: the serial without the target or the key, so that a
/// `changed` handler that sees the serial arrive does not take a press or key from before it as new
fn without_past(name: &str, value: &Value) -> Value {
    match name {
        "press" => PRESS.with_borrow(|(_, _, n)| press_value("", 0, *n)),
        "key" => KEY.with_borrow(|(_, _, n)| key_value("", false, *n)),
        _ => value.clone(),
    }
}

/// ComponentContainers in the window embed the components
pub const EMBEDS: bool = true;

pub fn factory(component: ComponentDefinition) -> ComponentFactory {
    ComponentFactory::new(move |ctx| {
        let instance = component
            .create_embedded(ctx)
            .map_err(|e| eprintln!("[ui] plugin component {}: {e}", component.name()))
            .ok()?;
        STATE.with_borrow(|state| {
            for (name, value) in state {
                let _ = instance.set_global_property(GLOBAL, name, without_past(name, value));
            }
        });
        let _ = instance.set_global_callback(GLOBAL, "action", |args| {
            let arg = |i: usize| match args.get(i) {
                Some(Value::String(s)) => s.to_string(),
                _ => String::new(),
            };
            super::action(arg(0), arg(1));
            Value::Void
        });
        let _ = instance.set_global_callback(GLOBAL, "emit", |args| {
            let arg = |i: usize| match args.get(i) {
                Some(Value::String(s)) => s.to_string(),
                _ => String::new(),
            };
            super::emit(&arg(0), &arg(1), &arg(2));
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

/// Each component connects its global itself, in its factory
pub fn connect(_ui: &AppWindow) {}

/// Hands what changed to all components on screen; each has the global for itself
pub fn publish(_ui: &AppWindow, changed: &[(&'static str, Value)]) {
    LIVE.with_borrow_mut(|live| {
        live.retain(|w| match w.upgrade() {
            Some(instance) => {
                for (name, value) in changed {
                    let _ = instance.set_global_property(GLOBAL, name, value.clone());
                }
                true
            }
            None => false,
        })
    });
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

/// Reads the manifest in `dir` and compiles the plugin; the error says what is wrong with it.
pub fn compile(dir: &Path) -> Result<Plugin, String> {
    let started = Instant::now();
    let manifest = plugin::manifest(
        &std::fs::read_to_string(dir.join("plugin.json"))
            .map_err(|e| format!("plugin.json: {e}"))?,
    )?;
    let file = plugin::text(&manifest, "file").ok_or("plugin.json names no \"file\"")?;
    let name = plugin::text(&manifest, "name").unwrap_or_else(|| file.clone());

    let mut compiler = Compiler::default();
    compiler.set_library_paths(HashMap::from([(
        "livi".to_string(),
        PathBuf::from(API_DIR),
    )]));
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
    // LIVI hands the state over through the global Livi; the interpreter only reaches a global the main file exports
    if !result
        .components()
        .next()
        .is_some_and(|c| c.globals().any(|g| g == GLOBAL))
    {
        return Err(format!(
            "{file} does not export the global Livi; add: export {{ Livi }} from \"@livi/api.slint\";"
        ));
    }
    let component = |name: &str| {
        result
            .component(name)
            .ok_or_else(|| format!("{file} exports no window component {name}"))
    };
    plugin::assemble(&manifest, name, started, component, |f| {
        Image::load_from_path(&dir.join(f)).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("plugin/example")
    }

    #[test]
    fn the_example_compiles_with_its_pages_and_bars() {
        let p = compile(&example()).expect("example plugin");
        assert_eq!(p.name, "Example");
        assert_eq!(p.start.as_deref(), Some("hello"));
        assert!(p.page("hello").is_some());
        assert!(p.shell.contains_key("bottom"));
        assert_eq!(p.fullscreen, vec!["/camera".to_string()]);
        assert_eq!(p.hide, vec!["/telemetry".to_string()]);
        let settings = p.page("settings").expect("settings page");
        assert_eq!(settings.settings, vec!["list".to_string()]);
        assert!(!settings.tab);
    }

    #[test]
    fn a_manifest_for_another_api_is_refused() {
        let dir = std::env::temp_dir().join(format!("livi-plugin-api-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{ "api": 99, "file": "x.slint" }"#,
        )
        .unwrap();
        let err = compile(&dir).err().expect("refused");
        assert!(err.contains("plugin API 99"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_page_needs_a_component_the_file_exports() {
        let dir = std::env::temp_dir().join(format!("livi-plugin-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{ "api": 1, "file": "x.slint", "pages": [{ "id": "a", "component": "Missing" }] }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("x.slint"),
            "export { Livi } from \"@livi/api.slint\";\nexport component Other inherits Window { in property <int> n; }",
        )
        .unwrap();
        let err = compile(&dir).err().expect("refused");
        assert!(err.contains("Missing"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_page_can_stay_out_of_the_tabs() {
        let dir = std::env::temp_dir().join(format!("livi-plugin-tab-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{ "api": 1, "file": "x.slint", "pages": [
                { "id": "a", "component": "Page" }, { "id": "b", "component": "Page", "tab": false } ] }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("x.slint"),
            "export { Livi } from \"@livi/api.slint\";\nexport component Page inherits Window { }",
        )
        .unwrap();
        let p = compile(&dir).expect("compiles");
        assert!(p.page("a").is_some_and(|a| a.tab));
        assert!(p.page("b").is_some_and(|b| !b.tab));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plugin_must_export_the_global_with_the_state() {
        let dir = std::env::temp_dir().join(format!("livi-plugin-global-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{ "api": 1, "file": "x.slint" }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("x.slint"),
            "import { Livi } from \"@livi/api.slint\";\nexport component Page inherits Window { Text { text: Livi.clock; } }",
        )
        .unwrap();
        let err = compile(&dir).err().expect("refused");
        assert!(err.contains("export { Livi }"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn field(v: &Value, name: &str) -> Value {
        match v {
            Value::Struct(s) => s.get_field(name).cloned().expect(name),
            _ => panic!("not a struct"),
        }
    }

    #[test]
    fn a_component_created_later_takes_no_press_or_key_from_before_as_new() {
        super::super::press("volume", 1);
        super::super::press("volume", 0);
        let [(_, now), _] = super::super::key_fields();
        assert_eq!(field(&now, "target"), state::s("volume"));
        assert_eq!(field(&now, "serial"), Value::Number(2.0));
        let fresh = without_past("press", &now);
        assert_eq!(field(&fresh, "target"), state::s(""));
        assert_eq!(field(&fresh, "serial"), Value::Number(2.0));
        super::super::unused_key("F13", true);
        let [_, (_, key)] = super::super::key_fields();
        assert_eq!(field(&key, "code"), state::s("F13"));
        assert_eq!(field(&key, "repeat"), Value::Bool(true));
        assert_eq!(field(&without_past("key", &key), "code"), state::s(""));
        let clock = state::s("12:00");
        assert_eq!(without_past("clock", &clock), clock);
    }
}
