//! LIVI's settings on a plugin page (docs/plugins.md, "settings" of a page in the manifest). The screen on top of
//! LIVI's settings stack goes to the plugin as state, its wishes go to the callbacks LIVI's own settings page uses,
//! so the logic, saving and translations stay LIVI's. A screen the page does not draw (e.g. a colour, a text, a
//! dialog) is shown on LIVI's own settings page, and the plugin's page comes back once the screen is one it draws.

use slint::{ComponentHandle as _, Model};
use slint_interpreter::Value;

use super::state::{model, record, s};
use crate::{AppWindow, RowKind, ScreenKind, SettingsState};

fn screen_name(k: ScreenKind) -> &'static str {
    match k {
        ScreenKind::List => "list",
        ScreenKind::Select => "select",
        ScreenKind::Number => "number",
        ScreenKind::Text => "text",
        ScreenKind::Color => "color",
        ScreenKind::Slider => "slider",
        ScreenKind::Calibration => "calibration",
        ScreenKind::Pending => "pending",
    }
}

fn row_name(k: RowKind) -> &'static str {
    match k {
        RowKind::Route => "route",
        RowKind::Value => "value",
        RowKind::Toggle => "toggle",
        RowKind::Slider => "slider",
        RowKind::KeyBinding => "key-binding",
        RowKind::Custom => "custom",
        RowKind::Devices => "devices",
        RowKind::Info => "info",
        RowKind::Button => "button",
        RowKind::Progress => "progress",
        RowKind::Item => "item",
        RowKind::Device => "device",
        RowKind::Picture => "picture",
        RowKind::Pick => "pick",
    }
}

/// What a settings page cannot show however it draws: a dialog or the wait for a key over LIVI's settings
fn interrupted(st: &SettingsState) -> bool {
    st.get_dialog_open() || st.get_capture_open()
}

/// Whether a page that draws `kinds` can show the screen LIVI's settings are on
pub fn drawable(ui: &AppWindow, kinds: &[String]) -> bool {
    let st = ui.global::<SettingsState>();
    !interrupted(&st) && kinds.iter().any(|k| k == screen_name(st.get_kind()))
}

/// The properties `settings`, `setting-rows` and `setting-picks` of the global Livi; empty while no page of the
/// plugin shows the settings (LIVI fills its settings state only then)
pub fn fields(ui: &AppWindow, shown: bool) -> Vec<(&'static str, Value)> {
    let st = ui.global::<SettingsState>();
    let number = |v: f32| Value::Number(f64::from(v));
    let rows = if shown {
        st.get_rows()
            .iter()
            .map(|r| {
                record(vec![
                    ("kind", s(row_name(r.kind))),
                    ("label", s(&r.label)),
                    ("icon", Value::Image(r.icon.clone())),
                    ("has-icon", Value::Bool(r.has_icon)),
                    ("value", s(&r.value)),
                    ("checked", Value::Bool(r.checked)),
                    ("level", number(r.level)),
                    ("enabled", Value::Bool(r.enabled)),
                    ("dimmed", Value::Bool(r.dimmed)),
                    ("alert", Value::Bool(r.alert)),
                    ("button", s(&r.button)),
                    ("node", Value::Number(f64::from(r.node))),
                ])
            })
            .collect()
    } else {
        Vec::new()
    };
    let picks = if shown {
        st.get_picks()
            .iter()
            .map(|p| {
                record(vec![
                    ("label", s(&p.label)),
                    ("selected", Value::Bool(p.selected)),
                ])
            })
            .collect()
    } else {
        Vec::new()
    };
    let screen = if shown {
        record(vec![
            ("kind", s(screen_name(st.get_kind()))),
            ("title", s(st.get_title())),
            ("can-back", Value::Bool(st.get_can_back())),
            ("show-apply", Value::Bool(st.get_show_apply())),
            ("apply-text", s(st.get_apply_text())),
            ("read-only", Value::Bool(st.get_read_only())),
            ("number-value", number(st.get_number_value())),
            ("number-text", s(st.get_number_text())),
            ("number-min", number(st.get_number_min())),
            ("number-max", number(st.get_number_max())),
            ("has-min", Value::Bool(st.get_number_has_min())),
            ("has-max", Value::Bool(st.get_number_has_max())),
            ("number-step", number(st.get_number_step())),
            ("level", number(st.get_level())),
            ("level-text", s(st.get_level_text())),
            ("pending-name", s(st.get_pending_name())),
        ])
    } else {
        record(vec![("kind", s(""))])
    };
    vec![
        ("settings", screen),
        ("setting-rows", model(rows)),
        ("setting-picks", model(picks)),
    ]
}

/// A wish of the plugin for the settings: `Livi.action("settings", "<verb>[:<a>[:<b>]]")`
#[derive(Debug, PartialEq)]
enum Wish {
    Open(i32),
    Toggle(i32),
    Press(i32),
    Capture(i32),
    Slide(i32, f32),
    SlideDone(i32, f32),
    Pick(i32),
    Number(f32),
    Level(f32),
    LevelDone(f32),
    Back,
    Apply,
}

fn parse(argument: &str) -> Option<Wish> {
    let mut parts = argument.split(':');
    let verb = parts.next()?;
    let a = parts.next();
    let b = parts.next();
    let int = |v: Option<&str>| v?.parse::<i32>().ok();
    let float = |v: Option<&str>| v?.parse::<f32>().ok().filter(|f| f.is_finite());
    Some(match verb {
        "open" => Wish::Open(int(a)?),
        "toggle" => Wish::Toggle(int(a)?),
        "press" => Wish::Press(int(a)?),
        "capture" => Wish::Capture(int(a)?),
        "slide" => Wish::Slide(int(a)?, float(b)?),
        "slide-done" => Wish::SlideDone(int(a)?, float(b)?),
        "pick" => Wish::Pick(int(a)?),
        "number" => Wish::Number(float(a)?),
        "level" => Wish::Level(float(a)?),
        "level-done" => Wish::LevelDone(float(a)?),
        "back" => Wish::Back,
        "apply" => Wish::Apply,
        _ => return None,
    })
}

/// Hands a wish of the plugin to LIVI's settings; false if it is not one
pub fn act(ui: &AppWindow, argument: &str) -> bool {
    let Some(wish) = parse(argument) else {
        return false;
    };
    let st = ui.global::<SettingsState>();
    match wish {
        Wish::Open(node) => st.invoke_open(node),
        Wish::Toggle(row) => st.invoke_toggle(row),
        Wish::Press(row) => st.invoke_press(row),
        Wish::Capture(row) => st.invoke_capture(row),
        Wish::Slide(node, level) => st.invoke_slide(node, level),
        Wish::SlideDone(node, level) => st.invoke_slide_done(node, level),
        Wish::Pick(index) => st.invoke_pick(index),
        Wish::Number(value) => st.invoke_number_set(value),
        Wish::Level(level) => st.invoke_level_moved(level),
        Wish::LevelDone(level) => st.invoke_level_done(level),
        Wish::Back => st.invoke_back(),
        Wish::Apply => st.invoke_apply(),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wishes_are_read_with_their_numbers() {
        assert_eq!(parse("open:12"), Some(Wish::Open(12)));
        assert_eq!(parse("toggle:3"), Some(Wish::Toggle(3)));
        assert_eq!(parse("slide:7:0.25"), Some(Wish::Slide(7, 0.25)));
        assert_eq!(parse("slide-done:7:1"), Some(Wish::SlideDone(7, 1.0)));
        assert_eq!(parse("number:-2.5"), Some(Wish::Number(-2.5)));
        assert_eq!(parse("back"), Some(Wish::Back));
    }

    #[test]
    fn broken_wishes_are_none() {
        for bad in [
            "",
            "open",
            "open:x",
            "slide:7",
            "slide:7:NaN",
            "level:inf",
            "erase:1",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn screen_kinds_have_the_names_of_the_manifest() {
        assert_eq!(screen_name(ScreenKind::List), "list");
        assert_eq!(screen_name(ScreenKind::Calibration), "calibration");
        assert_eq!(row_name(RowKind::KeyBinding), "key-binding");
    }
}
