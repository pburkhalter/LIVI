//! LIVI's settings on a plugin page (docs/plugins.md, "settings" of a page in the manifest). The screen on top of
//! LIVI's settings stack goes to the plugin as state, its wishes go to the callbacks LIVI's own settings page uses,
//! so the logic, saving and translations stay LIVI's. A screen the page does not draw (a kind it does not name, or a
//! dialog or the wait for a key without "dialog" or "capture") is shown on LIVI's own settings page, and the plugin's
//! page comes back once the screen is one it draws.

use slint::{Brush, Color, ComponentHandle as _, Model};
use slint_interpreter::Value;

use super::state::{model, record, s};
use crate::{AppWindow, RowData, RowKind, ScreenKind, SettingsState};

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

/// Whether a page that draws `kinds` can show the screen LIVI's settings are on, with its dialog or key wait
pub fn drawable(ui: &AppWindow, kinds: &[String]) -> bool {
    let st = ui.global::<SettingsState>();
    let draws = |kind: &str| kinds.iter().any(|k| k == kind);
    draws(screen_name(st.get_kind()))
        && (!st.get_dialog_open() || draws("dialog"))
        && (!st.get_capture_open() || draws("capture"))
}

fn color(c: Color) -> Value {
    Value::Brush(Brush::SolidColor(c))
}

fn row(r: &RowData) -> Value {
    record(vec![
        ("kind", s(row_name(r.kind))),
        ("label", s(&r.label)),
        ("icon", Value::Image(r.icon.clone())),
        ("has-icon", Value::Bool(r.has_icon)),
        ("value", s(&r.value)),
        ("checked", Value::Bool(r.checked)),
        ("level", Value::Number(f64::from(r.level))),
        ("enabled", Value::Bool(r.enabled)),
        ("dimmed", Value::Bool(r.dimmed)),
        ("alert", Value::Bool(r.alert)),
        ("button", s(&r.button)),
        ("node", Value::Number(f64::from(r.node))),
    ])
}

/// The properties `settings`, `setting-rows` and `setting-picks` of the global Livi; empty while no page of the
/// plugin shows the settings (LIVI fills its settings state only then)
pub fn fields(ui: &AppWindow, shown: bool) -> Vec<(&'static str, Value)> {
    let st = ui.global::<SettingsState>();
    let number = |v: f32| Value::Number(f64::from(v));
    let rows = |m: slint::ModelRc<RowData>| {
        if shown {
            m.iter().map(|r| row(&r)).collect()
        } else {
            Vec::new()
        }
    };
    let knobs = if shown {
        st.get_cal_knobs()
            .iter()
            .map(|k| {
                record(vec![
                    ("text", s(&k.text)),
                    ("icon", Value::Image(k.icon.clone())),
                    ("has-icon", Value::Bool(k.has_icon)),
                    ("tint", color(k.tint)),
                    ("has-tint", Value::Bool(k.has_tint)),
                    ("level", number(k.level)),
                    ("neutral", Value::Bool(k.neutral)),
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
            ("text-value", s(st.get_text_value())),
            ("text-max", Value::Number(f64::from(st.get_text_max()))),
            ("color-preview", color(st.get_color_preview())),
            ("color-hex", s(st.get_color_hex())),
            ("color-custom", Value::Bool(st.get_color_custom())),
            ("hue", number(st.get_color_hue())),
            ("saturation", number(st.get_color_saturation())),
            ("lightness", number(st.get_color_lightness())),
            ("saturation-from", color(st.get_color_sat_from())),
            ("saturation-to", color(st.get_color_sat_to())),
            ("lightness-mid", color(st.get_color_light_mid())),
            ("hue-label", s(st.get_hue_label())),
            ("saturation-label", s(st.get_saturation_label())),
            ("lightness-label", s(st.get_lightness_label())),
            ("wedge", Value::Bool(st.get_cal_wedge())),
            ("dialog-open", Value::Bool(st.get_dialog_open())),
            ("dialog-title", s(st.get_dialog_title())),
            ("capture-open", Value::Bool(st.get_capture_open())),
            ("capture-title", s(st.get_capture_title())),
            ("capture-hint", s(st.get_capture_hint())),
        ])
    } else {
        record(vec![("kind", s(""))])
    };
    vec![
        ("settings", screen),
        ("setting-rows", model(rows(st.get_rows()))),
        ("setting-picks", model(picks)),
        ("setting-knobs", model(knobs)),
        ("setting-dialog-rows", model(rows(st.get_dialog_rows()))),
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
    Text(String),
    Hex(String),
    ColorSlide(i32, f32),
    ColorSlideDone(i32, f32),
    ColorReset,
    Knob(i32, f32),
    KnobDone(i32, f32),
    KnobReset(i32),
    DialogPress(i32),
    CaptureCancel,
    Back,
    Apply,
}

fn parse(argument: &str) -> Option<Wish> {
    // a text may hold colons itself: everything after the first one
    if let Some(text) = argument.strip_prefix("text:") {
        return Some(Wish::Text(text.to_string()));
    }
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
        "hex" => Wish::Hex(a?.to_string()),
        "color-slide" => Wish::ColorSlide(int(a)?, float(b)?),
        "color-slide-done" => Wish::ColorSlideDone(int(a)?, float(b)?),
        "color-reset" => Wish::ColorReset,
        "knob" => Wish::Knob(int(a)?, float(b)?),
        "knob-done" => Wish::KnobDone(int(a)?, float(b)?),
        "knob-reset" => Wish::KnobReset(int(a)?),
        "dialog-press" => Wish::DialogPress(int(a)?),
        "capture-cancel" => Wish::CaptureCancel,
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
        Wish::Text(text) => st.invoke_text_commit(text.into()),
        Wish::Hex(hex) => st.invoke_color_hex_commit(hex.into()),
        Wish::ColorSlide(i, level) => st.invoke_color_slide(i, level),
        Wish::ColorSlideDone(i, level) => st.invoke_color_slide_done(i, level),
        Wish::ColorReset => st.invoke_color_reset(),
        Wish::Knob(i, level) => st.invoke_cal_moved(i, level),
        Wish::KnobDone(i, level) => st.invoke_cal_done(i, level),
        Wish::KnobReset(i) => st.invoke_cal_reset(i),
        Wish::DialogPress(i) => st.invoke_dialog_press(i),
        Wish::CaptureCancel => st.invoke_capture_cancel(),
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
        assert_eq!(
            parse("text:Audi A5: Sportback"),
            Some(Wish::Text("Audi A5: Sportback".into()))
        );
        assert_eq!(parse("text:"), Some(Wish::Text(String::new())));
        assert_eq!(parse("color-slide:2:0.5"), Some(Wish::ColorSlide(2, 0.5)));
        assert_eq!(parse("knob-reset:1"), Some(Wish::KnobReset(1)));
        assert_eq!(parse("dialog-press:0"), Some(Wish::DialogPress(0)));
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
