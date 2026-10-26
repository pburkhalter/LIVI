//! The state a plugin component sees in `livi` (`LiviState` in plugin/api.slint), built from LIVI's window and the
//! core's state. The telemetry parts are plain functions of the JSON, so they are tested here.

use std::rc::Rc;

use serde_json::Value as Json;
use slint::{ComponentHandle as _, Image, Model, ModelRc, SharedString, VecModel};
use slint_interpreter::{Struct, Value};

use super::plugin::{API, Plugin};
use crate::{AppWindow, Page, Theme};

fn num(v: &Json) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

fn s(text: impl AsRef<str>) -> Value {
    Value::String(SharedString::from(text.as_ref()))
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Struct(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<Struct>(),
    )
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

/// Path of a tab: "/plugin/<id>" for the plugin's pages, otherwise LIVI's route
pub fn tab_path(page: Page, plugin_id: &str) -> String {
    match page {
        Page::Plugin => format!("/plugin/{plugin_id}"),
        Page::Projection => "/projection".into(),
        Page::Telemetry => "/telemetry".into(),
        Page::Media => "/media".into(),
        Page::Camera => "/camera".into(),
        Page::Settings => "/settings".into(),
    }
}

fn pages(ui: &AppWindow, plugin: &Plugin, current: &str) -> Value {
    let out = ui
        .get_tabs()
        .iter()
        .map(|tab| {
            let path = tab_path(tab.page, &tab.plugin);
            let title = match tab.page {
                Page::Plugin => plugin
                    .page(&tab.plugin)
                    .map_or_else(|| tab.plugin.to_string(), |p| p.title.clone()),
                Page::Projection => "Projection".into(),
                Page::Telemetry => "Dashboard".into(),
                Page::Media => "Media".into(),
                Page::Camera => "Camera".into(),
                Page::Settings => "Settings".into(),
            };
            record(vec![
                ("active", Value::Bool(path == current)),
                ("path", s(&path)),
                ("title", s(title)),
                ("icon", Value::Image(tab.icon.clone())),
            ])
        })
        .collect();
    model(out)
}

fn media(ui: &AppWindow) -> Value {
    // LIVI's media page shows a dash for a missing text; plugins get an empty one
    let text = |t: SharedString| {
        s(if t.chars().all(|c| matches!(c, '-' | '—' | '–')) {
            ""
        } else {
            t.as_str()
        })
    };
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

fn nav(state: &Json, icon: impl Fn(&str) -> Image) -> Value {
    let n = livi_ui_model::dashboard::nav::nav(state);
    record(vec![
        ("active", Value::Bool(n.active)),
        ("icon", Value::Image(icon(n.icon))),
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
    let value = |key: &str| Value::Number(f(key).unwrap_or(0.0));
    let has = |key: &str| Value::Bool(f(key).is_some());
    let gear = match &t["gear"] {
        Json::String(g) => g.clone(),
        Json::Number(n) => n.to_string(),
        _ => String::new(),
    };
    record(vec![
        (
            "known",
            Value::Bool(t.as_object().is_some_and(|o| !o.is_empty())),
        ),
        ("speed-kph", value("speedKph")),
        ("has-speed", has("speedKph")),
        ("rpm", value("rpm")),
        ("has-rpm", has("rpm")),
        ("gear", s(gear)),
        ("ambient-c", value("ambientC")),
        ("has-ambient", has("ambientC")),
        ("fuel-pct", value("fuelPct")),
        ("has-fuel", has("fuelPct")),
        ("range-km", value("rangeKm")),
        ("has-range", has("rangeKm")),
        ("odometer-km", value("odometerKm")),
        ("trip-km", value("odometerTripKm")),
        ("reverse", b("reverse")),
        ("lights", b("lights")),
        ("high-beam", b("highBeam")),
        ("hazards", b("hazards")),
        ("parking-brake", b("parkingBrake")),
        ("night", b("nightMode")),
    ])
}

/// Climate as the car reports it. LIVI also keeps defaults and the values of the last run (source "default" or
/// "saved") for CarPlay's climate view; only values from outside (a telemetry source, source "external") are known.
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

/// Telemetry keys LIVI has no field for (e.g. doors from a CAN reader), as text
fn other(t: &Json) -> Value {
    let out = t
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !KNOWN.contains(&key.as_str()))
        .filter_map(|(key, value)| {
            let text = match value {
                Json::String(v) => v.clone(),
                Json::Null => return None,
                v => v.to_string(),
            };
            Some(record(vec![("key", s(key)), ("value", s(text))]))
        })
        .collect();
    model(out)
}

/// The whole `LiviState` for the page at `current`
pub fn build(ui: &AppWindow, plugin: &Plugin, state: &Json, current: &str, phone: &str) -> Value {
    let t = &state["telemetry"];
    let tm = local_time();
    record(vec![
        ("api", Value::Number(f64::from(API))),
        ("page", s(current)),
        ("clock", s(format!("{:02}:{:02}", tm.tm_hour, tm.tm_min))),
        ("date", date(&tm)),
        ("dark", Value::Bool(ui.global::<Theme>().get_dark())),
        ("phone", s(phone)),
        ("pages", pages(ui, plugin, current)),
        ("media", media(ui)),
        (
            "nav",
            nav(state, |name| crate::icons::icon(name).unwrap_or_default()),
        ),
        ("car", car(t)),
        ("climate", climate(&t["climate"])),
        ("other", other(t)),
    ])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn field(v: &Value, name: &str) -> Value {
        match v {
            Value::Struct(s) => s.get_field(name).cloned().expect(name),
            _ => panic!("not a struct"),
        }
    }

    #[test]
    fn climate_is_known_only_from_outside() {
        let from_car = climate(&json!({ "source": "external", "driverTargetC": 20.5 }));
        assert_eq!(field(&from_car, "known"), Value::Bool(true));
        assert_eq!(field(&from_car, "driver-c"), Value::Number(20.5));
        for source in ["default", "saved", "carplay"] {
            let kept = climate(&json!({ "source": source, "driverTargetC": 21.0, "auto": true }));
            assert_eq!(field(&kept, "known"), Value::Bool(false), "{source}");
        }
    }

    #[test]
    fn car_values_say_whether_they_arrived() {
        let c = car(&json!({ "speedKph": 84, "ambientC": -2.5 }));
        assert_eq!(field(&c, "has-speed"), Value::Bool(true));
        assert_eq!(field(&c, "speed-kph"), Value::Number(84.0));
        assert_eq!(field(&c, "ambient-c"), Value::Number(-2.5));
        assert_eq!(field(&c, "has-rpm"), Value::Bool(false));
        assert_eq!(field(&car(&json!({})), "known"), Value::Bool(false));
    }

    #[test]
    fn other_keeps_what_livi_has_no_field_for() {
        let o = other(&json!({ "speedKph": 1, "doors": "closed", "oilC": 92, "none": null }));
        let Value::Model(rows) = o else {
            panic!("not a model")
        };
        let keys: Vec<String> = rows
            .iter()
            .map(|r| match field(&r, "key") {
                Value::String(k) => k.to_string(),
                _ => String::new(),
            })
            .collect();
        assert!(keys.contains(&"doors".to_string()));
        assert!(keys.contains(&"oilC".to_string()));
        assert!(!keys.contains(&"speedKph".to_string()));
        assert!(!keys.contains(&"none".to_string()));
    }

    #[test]
    fn tabs_have_paths() {
        assert_eq!(tab_path(Page::Plugin, "home"), "/plugin/home");
        assert_eq!(tab_path(Page::Media, ""), "/media");
    }
}
