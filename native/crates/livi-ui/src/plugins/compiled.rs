//! The plugin built into LIVI (feature compiled-plugin, build_plugin.rs): its components are elements of the window, so
//! there is no interpreter, one global `Livi` for all of them, and nothing here that loads files.

use std::path::Path;
use std::time::Instant;

use slint::{
    Color, ComponentFactory, ComponentHandle as _, Image, Model, ModelRc, Rgba8Pixel,
    SharedPixelBuffer, SharedString, VecModel,
};

use super::plugin::{self, Plugin};
pub use super::value::{Struct, Value};
use crate::{AppWindow, Livi, LiviValue};

/// The window holds the components, the manifest's names are only checked by the build
#[derive(Clone)]
pub struct Component;

trait FromValue: Sized {
    fn from_value(value: &Value) -> Option<Self>;
}

fn from_value<T: FromValue + Default>(value: &Value) -> T {
    T::from_value(value).unwrap_or_default()
}

/// A field of a struct; the state leaves out fields that mean nothing at the moment
fn field<T: FromValue + Default>(value: &Value, name: &str) -> T {
    match value {
        Value::Struct(s) => s.get_field(name).map(from_value).unwrap_or_default(),
        _ => T::default(),
    }
}

impl FromValue for f32 {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(n) => Some(*n as f32),
            _ => None,
        }
    }
}

impl FromValue for i32 {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(n) => Some(*n as i32),
            _ => None,
        }
    }
}

impl FromValue for bool {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl FromValue for SharedString {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
    }
}

impl FromValue for Image {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Image(i) => Some(i.clone()),
            _ => None,
        }
    }
}

impl FromValue for Color {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Brush(b) => Some(b.color()),
            _ => None,
        }
    }
}

impl<T: FromValue + Clone + 'static> FromValue for ModelRc<T> {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Model(rows) => {
                let rows: Vec<T> = rows.iter().filter_map(|r| T::from_value(&r)).collect();
                Some(ModelRc::new(VecModel::from(rows)))
            }
            _ => None,
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/plugin_glue.rs"));

/// A picture the manifest names, built in
fn image(file: &str) -> Option<Image> {
    let (_, bytes) = ASSETS.iter().find(|(name, _)| *name == file)?;
    if file.ends_with(".svg") {
        return Image::load_from_svg_data(bytes).ok();
    }
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    );
    Some(Image::from_rgba8(pixels))
}

/// `LIVI --check-plugin`: the build has checked the one plugin there is
pub fn check(_dir: &Path) -> bool {
    println!("[ui] this LIVI has its plugin built in, the build checked it");
    true
}

pub fn load() -> Option<Plugin> {
    let started = Instant::now();
    let manifest = plugin::manifest(MANIFEST)
        .map_err(|e| eprintln!("[ui] built-in plugin: {e}"))
        .ok()?;
    let name = plugin::text(&manifest, "name").unwrap_or_else(|| "built in".into());
    plugin::assemble(&manifest, name, started, |_| Ok(Component), image)
        .map_err(|e| eprintln!("[ui] built-in plugin: {e}"))
        .ok()
}

/// The window holds the components, it asks no factory for them
pub const EMBEDS: bool = false;

pub fn factory(_component: Component) -> ComponentFactory {
    ComponentFactory::default()
}

/// Connects the callbacks of the global `Livi`
pub fn connect(ui: &AppWindow) {
    let livi = ui.global::<Livi>();
    livi.on_action(|verb, argument| super::action(verb.to_string(), argument.to_string()));
    livi.on_emit(|topic, name, value| super::emit(&topic, &name, &value));
    livi.on_find(|values: ModelRc<LiviValue>, key: SharedString| {
        values
            .iter()
            .find(|row| row.key == key)
            .map_or_else(SharedString::default, |row| row.value)
    });
}

/// Hands what changed to the global `Livi`
pub fn publish(ui: &AppWindow, changed: &[(&'static str, Value)]) {
    let livi = ui.global::<Livi>();
    for (name, value) in changed {
        set_property(&livi, name, value);
    }
}
