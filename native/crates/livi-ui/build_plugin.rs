//! Feature `compiled-plugin`: makes the plugin in the folder `LIVI_PLUGIN_DIR` part of LIVI's window at build time, so
//! that it runs as compiled code instead of through the Slint interpreter (docs/plugins.md, "Built in").
//!
//! Slint's generated Rust code cannot be embedded into a ComponentContainer (its `embed_component` refuses), so the
//! plugin's components become elements of the window: for the folder of `@plugin` (ui/plugin-hosted without this
//! feature) this writes `slots.slint`, whose components are the ones the manifest names, and `exports.slint`, which
//! says the plugin is built in and exports the global `Livi` to Rust. `plugin_glue.rs` hands the state from `plugins/value.rs` to the typed
//! setters of the global; it comes from plugin/api.slint, so the interface stays in that one file.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;

/// The plugin API this LIVI has; `API` in src/plugins/plugin.rs checks it again at start
const API: i64 = 1;

/// The places of the window: section and key in the manifest, component in the folder of `@plugin`
const PLACES: [(&str, &str, &str); 9] = [
    ("shell", "background", "PluginBackground"),
    ("shell", "start", "PluginStart"),
    ("shell", "end", "PluginEnd"),
    ("shell", "top", "PluginTop"),
    ("shell", "bottom", "PluginBottom"),
    ("shell", "overlay", "PluginOverlay"),
    ("empty", "projection", "PluginNoPhone"),
    ("empty", "camera", "PluginNoCamera"),
    ("frame", "camera", "PluginCameraFrame"),
];

/// The names in plugin/api.slint: the structs with their fields, the properties of the global `Livi`. The file has one
/// declaration per line.
#[derive(Debug, Default, PartialEq)]
struct Api {
    structs: Vec<(String, Vec<String>)>,
    properties: Vec<String>,
}

fn read_api(source: &str) -> Api {
    let mut api = Api::default();
    let (mut in_struct, mut in_global) = (false, false);
    for line in source.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix("export struct ")
            .and_then(|l| l.strip_suffix(" {"))
        {
            api.structs.push((name.to_string(), Vec::new()));
            in_struct = true;
        } else if line == "export global Livi {" {
            in_global = true;
        } else if line == "}" {
            (in_struct, in_global) = (false, false);
        } else if line.starts_with("//") {
            continue;
        } else if in_struct {
            if let (Some((field, _)), Some((_, fields))) =
                (line.split_once(':'), api.structs.last_mut())
            {
                fields.push(field.to_string());
            }
        } else if in_global
            && let Some(name) = line
                .strip_prefix("in property <")
                .and_then(|l| l.split_once("> "))
                .and_then(|(_, l)| l.strip_suffix(';'))
        {
            api.properties.push(name.to_string());
        }
    }
    api
}

fn rust_name(name: &str) -> String {
    name.replace('-', "_")
}

/// The Rust side of the interface: the state as `Value`s into the typed structs and setters Slint generates
fn glue(api: &Api, manifest: &Path, assets: &[(String, PathBuf)]) -> String {
    let mut out = format!("pub static MANIFEST: &str = include_str!({manifest:?});\n");
    out.push_str("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (name, path) in assets {
        writeln!(out, "    ({name:?}, include_bytes!({path:?})),").unwrap();
    }
    out.push_str("];\n");
    for (name, fields) in &api.structs {
        writeln!(
            out,
            "impl FromValue for crate::{name} {{\n    fn from_value(value: &Value) -> Option<Self> {{\n        Some(Self {{"
        )
        .unwrap();
        for f in fields {
            writeln!(out, "            {}: field(value, {f:?}),", rust_name(f)).unwrap();
        }
        out.push_str("        })\n    }\n}\n");
    }
    out.push_str(
        "pub fn set_property(livi: &crate::Livi<'_>, name: &str, value: &Value) {\n    match name {\n",
    );
    for p in &api.properties {
        writeln!(
            out,
            "        {p:?} => livi.set_{}(from_value(value)),",
            rust_name(p)
        )
        .unwrap();
    }
    out.push_str("        _ => {}\n    }\n}\n");
    out
}

/// The component the manifest names at `section.key`, if any
fn named<'a>(manifest: &'a Json, section: &str, key: &str) -> Option<&'a str> {
    manifest[section][key].as_str()
}

fn slots(manifest: &Json, main: &Path) -> String {
    let mut names: BTreeMap<&str, usize> = BTreeMap::new();
    for (section, key, _) in PLACES {
        if let Some(c) = named(manifest, section, key) {
            let next = names.len();
            names.entry(c).or_insert(next);
        }
    }
    let pages = manifest["pages"].as_array().cloned().unwrap_or_default();
    for p in &pages {
        let c = p["component"].as_str().expect("a page names no component");
        let next = names.len();
        names.entry(c).or_insert(next);
    }
    let imports: Vec<String> = names
        .iter()
        .map(|(c, i)| format!("{c} as Plugin{i}"))
        .collect();
    let mut out = String::from("// Generated by build_plugin.rs\n");
    writeln!(out, "import {{ {} }} from {main:?};\n", imports.join(", ")).unwrap();
    for (section, key, place) in PLACES {
        match named(manifest, section, key) {
            Some(c) => writeln!(
                out,
                "export component {place} inherits Plugin{} {{ }}",
                names[c]
            ),
            None => writeln!(out, "export component {place} {{ }}"),
        }
        .unwrap();
    }
    out.push_str(
        "\nexport component PluginPages {\n    in property <bool> shown;\n    in property <string> active;\n",
    );
    for p in &pages {
        let (c, id) = (p["component"].as_str().unwrap(), p["id"].as_str().unwrap());
        writeln!(
            out,
            "    Plugin{} {{\n        visible: root.shown && root.active == {id:?};\n        width: parent.width;\n        height: parent.height;\n    }}",
            names[c]
        )
        .unwrap();
    }
    out.push_str("}\n");
    out
}

/// The files the manifest names besides the Slint ones: icons of the pages and the start picture. The interpreter
/// reads them from the folder; a built-in plugin has no folder at run time.
fn assets(manifest: &Json, dir: &Path) -> Vec<(String, PathBuf)> {
    let pages = manifest["pages"].as_array().into_iter().flatten();
    let mut files: Vec<String> = pages
        .filter_map(|p| p["icon"].as_str())
        .chain(manifest["splash"].as_str())
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    files
        .into_iter()
        .map(|f| {
            let path = dir.join(&f);
            (f, path)
        })
        .collect()
}

/// Writes the files for the plugin in `LIVI_PLUGIN_DIR` and gives the folder for `@plugin`
pub fn generate(crate_dir: &Path, out_dir: &Path) -> PathBuf {
    println!("cargo:rerun-if-env-changed=LIVI_PLUGIN_DIR");
    let dir = std::env::var_os("LIVI_PLUGIN_DIR")
        .filter(|d| !d.is_empty())
        .expect("the feature compiled-plugin needs the plugin's folder in LIVI_PLUGIN_DIR");
    let dir = fs::canonicalize(&dir).expect("LIVI_PLUGIN_DIR");
    let manifest_path = dir.join("plugin.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let text = fs::read_to_string(&manifest_path).expect("plugin.json");
    let manifest: Json = serde_json::from_str(&text).expect("plugin.json");
    assert_eq!(
        manifest["api"].as_i64(),
        Some(API),
        "plugin API of the plugin"
    );
    let main = dir.join(
        manifest["file"]
            .as_str()
            .expect("plugin.json names no \"file\""),
    );

    let folder = out_dir.join("plugin-compiled");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("slots.slint"), slots(&manifest, &main)).unwrap();

    let api_path = crate_dir.join("plugin/api.slint");
    let api = read_api(&fs::read_to_string(&api_path).expect("plugin/api.slint"));
    let names: Vec<&str> = api.structs.iter().map(|(n, _)| n.as_str()).collect();
    fs::write(
        folder.join("exports.slint"),
        format!(
            "// Generated by build_plugin.rs\nexport global PluginBuild {{\n    out property <bool> built-in: true;\n}}\nexport {{ Livi, {} }} from \"@livi/api.slint\";\n",
            names.join(", ")
        ),
    )
    .unwrap();
    fs::write(
        out_dir.join("plugin_glue.rs"),
        glue(&api, &manifest_path, &assets(&manifest, &dir)),
    )
    .unwrap();
    folder
}
