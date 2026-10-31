use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

#[cfg(feature = "compiled-plugin")]
#[path = "build_plugin.rs"]
mod plugin;

/// The folder of `@plugin` (ui/app.slint): how the window gets its plugin
#[cfg(feature = "compiled-plugin")]
fn plugin_places(crate_dir: &Path, out_dir: &Path) -> PathBuf {
    plugin::generate(crate_dir, out_dir)
}

#[cfg(not(feature = "compiled-plugin"))]
fn plugin_places(crate_dir: &Path, _out_dir: &Path) -> PathBuf {
    crate_dir.join("ui/plugin-hosted")
}

fn main() {
    // ComponentContainer and the component-factory type (UI plugins, src/plugins/) are experimental in Slint 1.18.
    // SAFETY: the build script sets it before it starts any thread.
    unsafe { std::env::set_var("SLINT_ENABLE_EXPERIMENTAL_FEATURES", "1") };
    let crate_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let libraries = HashMap::from([
        ("livi".to_string(), crate_dir.join("plugin")),
        ("plugin".to_string(), plugin_places(&crate_dir, &out_dir)),
    ]);
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_library_paths(libraries),
    )
    .unwrap();

    let dir = crate_dir.join("ui/icons");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".svg").map(str::to_string))
        .collect();
    names.sort();
    let mut table = String::from("pub static ICONS: &[(&str, &[u8])] = &[\n");
    for n in &names {
        let path = dir.join(format!("{n}.svg"));
        writeln!(
            table,
            "    ({n:?}, include_bytes!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    table.push_str("];\n");
    std::fs::write(out_dir.join("icons.rs"), table).unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!(
            "cargo:rustc-link-arg=-Wl,-rpath,/Library/Frameworks/GStreamer.framework/Versions/1.0/lib"
        );
    }
}
