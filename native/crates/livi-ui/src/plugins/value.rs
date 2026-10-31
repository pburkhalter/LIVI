//! The state of the global `Livi` without the interpreter: the part of `slint_interpreter::Value` that state.rs and
//! settings.rs use, so that they are the same for both ways to get a plugin. compiled.rs turns it into the types Slint
//! generates.

use slint::{Brush, Image, ModelRc, SharedString};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    String(SharedString),
    Bool(bool),
    Image(Image),
    Brush(Brush),
    Model(ModelRc<Value>),
    Struct(Struct),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Struct(Vec<(String, Value)>);

impl Struct {
    pub fn get_field(&self, name: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl FromIterator<(String, Value)> for Struct {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(fields: I) -> Self {
        Self(fields.into_iter().collect())
    }
}
