//! A plugin as LIVI knows it: what its manifest (`plugin.json`) says, with its components. Where the components come
//! from is up to the backend: the Slint interpreter at start (interpreted.rs) or the build (compiled.rs).

use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value as Json;
use slint::Image;

use super::backend::Component;

pub const API: i32 = 1;

/// The places around the pages a plugin can fill
pub const SLOTS: [&str; 6] = ["background", "start", "end", "top", "bottom", "overlay"];
/// LIVI's pages whose empty state a plugin can draw ("empty" in the manifest)
pub const EMPTY: [&str; 2] = ["projection", "camera"];
/// LIVI's pages whose picture a plugin can frame: it lays out the page around it and says where the picture goes
/// (action camera-area); "frame" in the manifest
pub const FRAME: [&str; 1] = ["camera"];

pub struct PluginPage {
    pub id: String,
    pub title: String,
    pub icon: Image,
    pub component: Component,
    /// among the tabs (and so in the order for swiping); a page only opened from the plugin's own buttons is not
    pub tab: bool,
    /// the kinds of LIVI's settings screens the page draws (settings.rs); empty: not a settings page
    pub settings: Vec<String>,
}

pub struct Plugin {
    pub name: String,
    pub compiled_ms: u128,
    pub start: Option<String>,
    pub splash: Option<Image>,
    /// pages shown without the bars, e.g. "/camera"
    pub fullscreen: Vec<String>,
    /// LIVI's pages the plugin replaces: not among the tabs, still reachable by path, e.g. "/media"
    pub hide: Vec<String>,
    pub shell: HashMap<&'static str, Component>,
    /// what the plugin shows on LIVI's pages while they have nothing to show, by page: "projection", "camera"
    pub empty: HashMap<&'static str, Component>,
    /// what the plugin draws around LIVI's picture while it shows, by page: "camera"
    pub frame: HashMap<&'static str, Component>,
    pub pages: Vec<PluginPage>,
}

impl Plugin {
    pub fn page(&self, id: &str) -> Option<&PluginPage> {
        self.pages.iter().find(|p| p.id == id)
    }
}

pub fn text(v: &Json, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Reads a manifest; the error says what is wrong with it.
pub fn manifest(text: &str) -> Result<Json, String> {
    let manifest: Json = serde_json::from_str(text).map_err(|e| format!("plugin.json: {e}"))?;
    let api = manifest["api"].as_i64().unwrap_or(0);
    if api != i64::from(API) {
        return Err(format!("written for plugin API {api}, LIVI has {API}"));
    }
    Ok(manifest)
}

/// The plugin of a manifest, with the components `component` gives for their names and the pictures `image` gives for
/// their files
pub fn assemble(
    manifest: &Json,
    name: String,
    started: Instant,
    component: impl Fn(&str) -> Result<Component, String>,
    image: impl Fn(&str) -> Option<Image>,
) -> Result<Plugin, String> {
    let mut shell = HashMap::new();
    for slot in SLOTS {
        if let Some(c) = manifest["shell"][slot].as_str() {
            shell.insert(slot, component(c)?);
        }
    }
    let mut empty = HashMap::new();
    for page in EMPTY {
        if let Some(c) = manifest["empty"][page].as_str() {
            empty.insert(page, component(c)?);
        }
    }
    let mut frame = HashMap::new();
    for page in FRAME {
        if let Some(c) = manifest["frame"][page].as_str() {
            frame.insert(page, component(c)?);
        }
    }
    let mut pages = Vec::new();
    for p in manifest["pages"].as_array().into_iter().flatten() {
        let id = text(p, "id").ok_or("a page without \"id\"")?;
        let component =
            component(&text(p, "component").ok_or(format!("page {id} names no \"component\""))?)?;
        let icon = text(p, "icon").and_then(|i| image(&i)).unwrap_or_default();
        pages.push(PluginPage {
            title: text(p, "title").unwrap_or_else(|| id.clone()),
            tab: p["tab"].as_bool().unwrap_or(true),
            settings: p["settings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect(),
            id,
            icon,
            component,
        });
    }
    let start = text(manifest, "start").filter(|s| pages.iter().any(|p| p.id == *s));
    let splash = text(manifest, "splash").and_then(|f| image(&f));
    let paths = |key: &str| -> Vec<String> {
        manifest[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str().map(str::to_string))
            .collect()
    };
    let (fullscreen, hide) = (paths("fullscreen"), paths("hide"));
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
        hide,
        shell,
        empty,
        frame,
        pages,
    })
}
