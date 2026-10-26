//! A plugin on disk: its manifest (`plugin.json`) and its `.slint` files, compiled with the Slint interpreter.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use serde_json::Value as Json;
use slint::Image;
use slint_interpreter::{Compiler, ComponentDefinition};

pub const API: i32 = 1;
const API_SOURCE: &str = include_str!("../../plugin/api.slint");
// Virtual folder for "@livi/api.slint"; the file loader answers for it
const API_DIR: &str = "/livi-plugin-api";

/// The places around the pages a plugin can fill
pub const SLOTS: [&str; 5] = ["background", "start", "end", "top", "bottom"];

pub struct PluginPage {
    pub id: String,
    pub title: String,
    pub icon: Image,
    pub component: ComponentDefinition,
}

pub struct Plugin {
    pub name: String,
    pub compiled_ms: u128,
    pub start: Option<String>,
    pub splash: Option<Image>,
    /// pages shown without the bars, e.g. "/camera"
    pub fullscreen: Vec<String>,
    pub shell: HashMap<&'static str, ComponentDefinition>,
    pub pages: Vec<PluginPage>,
}

impl Plugin {
    pub fn page(&self, id: &str) -> Option<&PluginPage> {
        self.pages.iter().find(|p| p.id == id)
    }
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
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Reads the manifest in `dir` and compiles the plugin; the error says what is wrong with it.
pub fn compile(dir: &Path) -> Result<Plugin, String> {
    let started = Instant::now();
    let manifest: Json = serde_json::from_str(
        &std::fs::read_to_string(dir.join("plugin.json"))
            .map_err(|e| format!("plugin.json: {e}"))?,
    )
    .map_err(|e| format!("plugin.json: {e}"))?;
    let api = manifest["api"].as_i64().unwrap_or(0);
    if api != i64::from(API) {
        return Err(format!("written for plugin API {api}, LIVI has {API}"));
    }
    let file = text(&manifest, "file").ok_or("plugin.json names no \"file\"")?;
    let name = text(&manifest, "name").unwrap_or_else(|| file.clone());

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
    let component = |name: &str| {
        result
            .component(name)
            .ok_or_else(|| format!("{file} exports no window component {name}"))
    };

    let mut shell = HashMap::new();
    for slot in SLOTS {
        if let Some(c) = manifest["shell"][slot].as_str() {
            shell.insert(slot, component(c)?);
        }
    }
    let mut pages = Vec::new();
    for p in manifest["pages"].as_array().into_iter().flatten() {
        let id = text(p, "id").ok_or("a page without \"id\"")?;
        let component =
            component(&text(p, "component").ok_or(format!("page {id} names no \"component\""))?)?;
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
    let compiled_ms = started.elapsed().as_millis();
    println!(
        "[ui] plugin {name}: {} pages, bars {:?}, compiled in {compiled_ms} ms",
        pages.len(),
        shell.keys().collect::<Vec<_>>(),
    );
    Ok(Plugin {
        name,
        compiled_ms,
        start,
        splash,
        fullscreen,
        shell,
        pages,
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
            "export component Other inherits Window { in property <int> n; }",
        )
        .unwrap();
        let err = compile(&dir).err().expect("refused");
        assert!(err.contains("Missing"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
