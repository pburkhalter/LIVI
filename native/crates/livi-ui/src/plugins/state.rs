//! The state a plugin sees in the global `Livi` (plugin/api.slint), built from LIVI's window and the core's state.
//! The telemetry parts are plain functions of the JSON, so they are tested here.

use std::rc::Rc;

use serde_json::Value as Json;
use slint::{ComponentHandle as _, Image, Model, ModelRc, SharedString, VecModel};
use slint_interpreter::{Struct, Value};

use super::plugin::Plugin;
use crate::settings_ui::tr;
use crate::{AppWindow, Page, Theme};

fn num(v: &Json) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

pub(super) fn s(text: impl AsRef<str>) -> Value {
    Value::String(SharedString::from(text.as_ref()))
}

pub(super) fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Struct(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<Struct>(),
    )
}

pub(super) fn model(items: Vec<Value>) -> Value {
    Value::Model(ModelRc::from(Rc::new(VecModel::from(items))))
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
                Page::Projection => tr("settings.startPageHome", "Home"),
                Page::Telemetry => tr("settings.dashboard", "Dashboard"),
                Page::Media => tr("settings.startPageMedia", "Media"),
                Page::Camera => tr("settings.startPageCamera", "Camera"),
                Page::Settings => tr("settings.startPageSettings", "Settings"),
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
        ("total-ms", Value::Number(f64::from(ui.get_total_ms()))),
    ])
}

/// Where the song is: apart from `media`, as it changes every second while it plays
fn playback(ui: &AppWindow) -> Value {
    record(vec![
        ("progress", Value::Number(f64::from(ui.get_progress()))),
        ("elapsed-ms", Value::Number(f64::from(ui.get_elapsed_ms()))),
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

fn number(t: &Json, key: &str) -> Value {
    Value::Number(num(&t[key]).unwrap_or(0.0))
}

fn has(t: &Json, key: &str) -> Value {
    Value::Bool(num(&t[key]).is_some())
}

/// What changes many times a second while driving: apart from `car`, so that reading `car` costs nothing then
fn drive(t: &Json) -> Value {
    let gear = match &t["gear"] {
        Json::String(g) => g.clone(),
        Json::Number(n) => n.to_string(),
        _ => String::new(),
    };
    record(vec![
        ("speed-kph", number(t, "speedKph")),
        ("has-speed", has(t, "speedKph")),
        ("rpm", number(t, "rpm")),
        ("has-rpm", has(t, "rpm")),
        ("gear", s(gear)),
    ])
}

fn car(t: &Json) -> Value {
    let b = |key: &str| Value::Bool(t[key].as_bool().unwrap_or(false));
    let value = |key: &str| number(t, key);
    let has = |key: &str| has(t, key);
    record(vec![
        (
            "known",
            Value::Bool(t.as_object().is_some_and(|o| !o.is_empty())),
        ),
        (
            "moving",
            Value::Bool(num(&t["speedKph"]).is_some_and(|v| v > 0.0)),
        ),
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

/// The notices other programs sent (org.freedesktop.Notifications, served by the core), newest first, with the
/// local time they came
fn notices(state: &Json, now: &libc::tm) -> Value {
    let out = state["notices"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .map(|n| {
            let text = |key: &str| s(n[key].as_str().unwrap_or_default());
            let tm =
                crate::clock::local_at((n["time"].as_u64().unwrap_or(0) / 1000) as libc::time_t);
            let urgency = match n["urgency"].as_u64() {
                Some(0) => "low",
                Some(2) => "critical",
                _ => "normal",
            };
            record(vec![
                ("id", Value::Number(n["id"].as_f64().unwrap_or(0.0))),
                ("serial", Value::Number(n["serial"].as_f64().unwrap_or(0.0))),
                ("app", text("app")),
                ("summary", text("summary")),
                ("body", text("body")),
                ("urgency", s(urgency)),
                (
                    "today",
                    Value::Bool(
                        (tm.tm_year, tm.tm_mon, tm.tm_mday)
                            == (now.tm_year, now.tm_mon, now.tm_mday),
                    ),
                ),
                ("hour", Value::Number(f64::from(tm.tm_hour))),
                ("minute", Value::Number(f64::from(tm.tm_min))),
                ("day", Value::Number(f64::from(tm.tm_mday))),
                ("month", Value::Number(f64::from(tm.tm_mon + 1))),
            ])
        })
        .collect();
    model(out)
}

/// How many notices are critical (a binding cannot count in a list)
fn critical(state: &Json) -> Value {
    let n = state["notices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["urgency"].as_u64() == Some(2))
        .count();
    Value::Number(n as f64)
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

/// Rows of key and value as text, for `other` and `can`
fn values<'a>(entries: impl Iterator<Item = (&'a String, &'a Json)>) -> Value {
    let out = entries
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

/// Telemetry keys LIVI has no field for (e.g. doors from a CAN reader), as text
fn other(t: &Json) -> Value {
    values(
        t.as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| !KNOWN.contains(&key.as_str())),
    )
}

/// What a CAN reader keeps under the telemetry key `can`: raw values that may change many times a second
fn can(t: &Json) -> Value {
    values(t["can"].as_object().into_iter().flatten())
}

/// What programs beside LIVI publish (companion channel), a row per key: "<topic>.<key>", the keys of a nested
/// object as "<topic>.<key>.<key>" and the object itself as JSON text
fn extensions(state: &Json) -> Value {
    fn flatten(
        prefix: &str,
        fields: &serde_json::Map<String, Json>,
        rows: &mut Vec<(String, Json)>,
    ) {
        for (k, v) in fields {
            let key = format!("{prefix}.{k}");
            if let Json::Object(inner) = v {
                flatten(&key, inner, rows);
            }
            rows.push((key, v.clone()));
        }
    }
    let mut rows = Vec::new();
    for (topic, value) in state["extensions"].as_object().into_iter().flatten() {
        if let Json::Object(fields) = value {
            flatten(topic, fields, &mut rows);
        }
    }
    values(rows.iter().map(|(k, v)| (k, v)))
}

/// The value of `key` among the `other` values (LiviValue rows), "" when there is none: `Livi.value(key)`
pub fn find(values: &ModelRc<Value>, key: &str) -> Value {
    let field = |row: &Value, name: &str| match row {
        Value::Struct(s) => s.get_field(name).cloned(),
        _ => None,
    };
    values
        .iter()
        .find(|row| field(row, "key") == Some(s(key)))
        .and_then(|row| field(&row, "value"))
        .unwrap_or_else(|| s(""))
}

/// Whether two state values are equal. Models compare by their rows: `ModelRc` compares by pointer, and the state gets
/// new models each time it is built. Two empty images count as equal: Slint's `Image` has no case for them in its
/// `PartialEq`, so a missing cover alone would make every state new.
pub fn same(a: &Value, b: &Value) -> bool {
    let empty = |i: &Image| i.size().width == 0 || i.size().height == 0;
    match (a, b) {
        (Value::Image(x), Value::Image(y)) => x == y || (empty(x) && empty(y)),
        (Value::Model(x), Value::Model(y)) => {
            x.row_count() == y.row_count() && x.iter().zip(y.iter()).all(|(p, q)| same(&p, &q))
        }
        (Value::Struct(x), Value::Struct(y)) => {
            x.iter().count() == y.iter().count()
                && x.iter()
                    .all(|(k, v)| y.get_field(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// The properties of the global `Livi` for the page at `current`, by name
pub fn build(
    ui: &AppWindow,
    plugin: &Plugin,
    state: &Json,
    current: &str,
    phone: &str,
) -> Vec<(&'static str, Value)> {
    let t = &state["telemetry"];
    let tm = crate::clock::local_now();
    let settings_shown = plugin
        .pages
        .iter()
        .any(|p| !p.settings.is_empty() && current == tab_path(Page::Plugin, &p.id));
    let mut fields = vec![
        ("page", s(current)),
        ("clock", s(format!("{:02}:{:02}", tm.tm_hour, tm.tm_min))),
        ("date", date(&tm)),
        ("dark", Value::Bool(ui.global::<Theme>().get_dark())),
        ("phone", s(phone)),
        ("pages", pages(ui, plugin, current)),
        ("media", media(ui)),
        ("playback", playback(ui)),
        (
            "nav",
            nav(state, |name| crate::icons::icon(name).unwrap_or_default()),
        ),
        ("drive", drive(t)),
        ("car", car(t)),
        ("climate", climate(&t["climate"])),
        ("other", other(t)),
        ("can", can(t)),
        ("extensions", extensions(state)),
        ("notices", notices(state, &tm)),
        ("notices-critical", critical(state)),
    ];
    fields.extend(super::settings::fields(ui, settings_shown));
    fields
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
        let t = json!({ "speedKph": 84, "ambientC": -2.5 });
        let d = drive(&t);
        assert_eq!(field(&d, "has-speed"), Value::Bool(true));
        assert_eq!(field(&d, "speed-kph"), Value::Number(84.0));
        assert_eq!(field(&d, "has-rpm"), Value::Bool(false));
        let c = car(&t);
        assert_eq!(field(&c, "ambient-c"), Value::Number(-2.5));
        assert_eq!(field(&c, "has-fuel"), Value::Bool(false));
        assert_eq!(field(&car(&json!({})), "known"), Value::Bool(false));
    }

    #[test]
    fn driving_leaves_the_car_values_as_they_were() {
        let at = |speed: i32| json!({ "speedKph": speed, "rpm": speed * 30, "ambientC": 4 });
        assert!(same(&car(&at(50)), &car(&at(80))));
        assert!(!same(&drive(&at(50)), &drive(&at(80))));
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
    fn raw_can_values_are_apart_from_the_others() {
        let t = json!({ "can": { "doors": 4, "boxVersion": "RZ 1.2" }, "trunkOpen": true });
        let (Value::Model(raw), Value::Model(others)) = (can(&t), other(&t)) else {
            panic!("not a model")
        };
        assert_eq!(find(&raw, "doors"), s("4"));
        assert_eq!(find(&raw, "boxVersion"), s("RZ 1.2"));
        assert_eq!(find(&others, "trunkOpen"), s("true"));
        assert_eq!(find(&others, "can"), s(""));
    }

    #[test]
    fn the_car_moves_with_any_speed() {
        for (speed, moving) in [(json!(0), false), (json!(1), true), (json!(null), false)] {
            let c = car(&json!({ "speedKph": speed }));
            assert_eq!(field(&c, "moving"), Value::Bool(moving), "{speed}");
        }
    }

    #[test]
    fn a_state_built_again_from_the_same_data_is_the_same() {
        let state = |cover: Image| {
            record(vec![
                ("media", record(vec![("artwork", Value::Image(cover))])),
                ("other", other(&json!({ "doors": "closed" }))),
            ])
        };
        assert!(same(&state(Image::default()), &state(Image::default())));
        let changed = record(vec![
            (
                "media",
                record(vec![("artwork", Value::Image(Image::default()))]),
            ),
            ("other", other(&json!({ "doors": "open" }))),
        ]);
        assert!(!same(&state(Image::default()), &changed));
    }

    #[test]
    fn a_value_is_found_by_its_key() {
        let Value::Model(values) = other(&json!({ "doors": "closed", "updates": 3 })) else {
            panic!("not a model")
        };
        assert_eq!(find(&values, "updates"), s("3"));
        assert_eq!(find(&values, "doors"), s("closed"));
        assert_eq!(find(&values, "trunk"), s(""));
    }

    #[test]
    fn what_programs_publish_is_found_by_topic_and_key() {
        let state = json!({ "extensions": {
            "voice": { "state": "listening", "level": 0.5, "keySet": false, "text": null,
                       "keyEntry": { "url": "http://10.0.0.1/", "code": "123456" } },
            "radio": { "station": "SRF 3" },
        } });
        let Value::Model(rows) = extensions(&state) else {
            panic!("not a model")
        };
        assert_eq!(find(&rows, "voice.state"), s("listening"));
        assert_eq!(find(&rows, "voice.level"), s("0.5"));
        assert_eq!(find(&rows, "voice.keySet"), s("false"));
        assert_eq!(find(&rows, "voice.text"), s(""));
        assert_eq!(find(&rows, "voice.keyEntry.code"), s("123456"));
        assert_ne!(find(&rows, "voice.keyEntry"), s(""));
        assert_eq!(find(&rows, "radio.station"), s("SRF 3"));
        assert_eq!(find(&rows, "radio.song"), s(""));
        assert!(same(&extensions(&json!({})), &model(Vec::new())));
    }

    #[test]
    fn notices_come_newest_first_with_their_urgency() {
        let now = crate::clock::local_now();
        let state = json!({ "notices": [
            { "id": 7, "app": "t100-updates", "summary": "3 updates", "body": "", "urgency": 2, "time": 0 },
            { "id": 9, "app": "radio", "summary": "on air", "body": "x", "urgency": 0, "time": 0 },
        ] });
        let Value::Model(rows) = notices(&state, &now) else {
            panic!("not a model")
        };
        let first = rows.row_data(0).unwrap();
        assert_eq!(field(&first, "id"), Value::Number(9.0));
        assert_eq!(field(&first, "urgency"), s("low"));
        assert_eq!(field(&rows.row_data(1).unwrap(), "urgency"), s("critical"));
        assert!(same(&notices(&json!({}), &now), &model(Vec::new())));
        assert_eq!(critical(&state), Value::Number(1.0));
    }

    #[test]
    fn tabs_have_paths() {
        assert_eq!(tab_path(Page::Plugin, "home"), "/plugin/home");
        assert_eq!(tab_path(Page::Media, ""), "/media");
    }
}
