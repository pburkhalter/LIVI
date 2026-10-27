use std::collections::BTreeSet;

use livi_core_proto::state::Front;
use livi_ui_model::shell::{Look, look};
use serde_json::Value;

use crate::PhoneState;
use crate::dashboard_ui::{DashReader, DashView};
use crate::media::{DabView, MediaReader, MediaView};
use livi_ui_model::dashboard::screens::secondary;
use livi_ui_model::mask::{Mask, Stream, mask};
use livi_ui_model::overlay;

pub enum Update {
    Media(MediaView),
    Dab(DabView),
    Dash(Box<DashView>),
    DashOn(livi_core_proto::state::Screen, Box<DashView>),
    Secondary(Vec<livi_ui_model::dashboard::screens::Secondary>),
    Front(Front),
    Phone(PhoneState),
    Look(Look),
    Kiosk(bool),
    Settings,
    Spectrum(Vec<f32>),
    Status(String),
    Open(String),
    Volume(f64),
    PhoneName(String),
    Camera(bool),
    MainMask(Option<Mask>),
    VideoCard(VideoCard),
}

/// Whether the phone's picture shows as a card now, and the card: distances from the edges of the phone's picture
/// and corner radius, in its pixels (all 0: no card, or one that does not fit the picture); Android Auto has no card
/// to draw in and stays full screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoCard {
    pub projection: bool,
    pub android: bool,
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
    pub radius: u32,
}

impl VideoCard {
    fn read(state: &Value) -> Self {
        let n = |key: &str| {
            state["config"][key]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0)
        };
        let android = state["sessions"]["active"].as_str() == Some("androidauto");
        // the same card the core gives the phone (Config::video_card)
        let card = [
            n("videoCardTop"),
            n("videoCardBottom"),
            n("videoCardLeft"),
            n("videoCardRight"),
        ];
        let [top, bottom, left, right] = if livi_core_proto::config::video_card_fits(
            card,
            n("projectionWidth"),
            n("projectionHeight"),
        ) {
            card
        } else {
            [0; 4]
        };
        Self {
            projection: state["video"]["projectionCard"].as_bool().unwrap_or(false) && !android,
            android,
            top,
            bottom,
            left,
            right,
            radius: n("videoCardRadius"),
        }
    }
}

#[derive(Default)]
pub struct Follow {
    media: MediaReader,
    front: Option<Front>,
    phone: Option<PhoneState>,
    look: Option<Look>,
    kiosk: Option<bool>,
    dash: DashReader,
    others: Vec<(livi_core_proto::state::Screen, DashReader)>,
    secondary: Option<Vec<livi_ui_model::dashboard::screens::Secondary>>,
    volume: overlay::Volume,
    phones: overlay::Phone,
    started: bool,
    main_mask: Option<Option<Mask>>,
    video_card: Option<VideoCard>,
    // start page of a UI plugin instead of LIVI's (read on the UI thread, Follow runs on the client's)
    start: Option<String>,
}

impl Follow {
    pub fn starting_at(start: Option<String>) -> Self {
        Self {
            start,
            ..Self::default()
        }
    }

    pub fn changed(&mut self, keys: Option<&BTreeSet<String>>, state: &Value) -> Vec<Update> {
        let all = keys.is_none_or(|k| k.contains(""));
        let hit = |key: &str| all || keys.is_some_and(|k| k.contains(key));
        let mut out = Vec::new();
        if hit("nowPlaying") || hit("sessions") {
            out.push(Update::Media(self.media.read(state)));
        }
        if hit("dab")
            && let Some(d) = self.media.read_dab(state)
        {
            out.push(Update::Dab(d));
        }
        if hit("sessions") {
            let phone = if state["sessions"]["active"].is_null() {
                PhoneState::None
            } else {
                PhoneState::Streaming
            };
            if self.phone != Some(phone) {
                self.phone = Some(phone);
                out.push(Update::Phone(phone));
            }
        }
        if hit("front") {
            let front: Option<Front> = serde_json::from_value(state["front"]["main"].clone()).ok();
            if let Some(front) = front
                && self.front != Some(front)
            {
                self.front = Some(front);
                out.push(Update::Front(front));
            }
        }
        if hit("config") {
            let next = look(&state["config"]);
            if self.look.as_ref() != Some(&next) {
                self.look = Some(next.clone());
                out.push(Update::Look(next));
            }
            let kiosk = state["config"]["kiosk"]["main"].as_bool().unwrap_or(false);
            if self.kiosk != Some(kiosk) {
                self.kiosk = Some(kiosk);
                out.push(Update::Kiosk(kiosk));
            }
        }
        if hit("video") || hit("config") || hit("sessions") {
            let next = VideoCard::read(state);
            if self.video_card != Some(next) {
                self.video_card = Some(next);
                out.push(Update::VideoCard(next));
            }
        }
        if hit("sessions") || hit("config") {
            let next = mask(state, Stream::Main);
            if self.main_mask != Some(next) {
                self.main_mask = Some(next);
                out.push(Update::MainMask(next));
            }
        }
        if hit("telemetry")
            || hit("config")
            || hit("cluster")
            || hit("navigation")
            || hit("sessions")
        {
            let dark = self.look.as_ref().is_none_or(|l| l.dark);
            out.push(Update::Dash(Box::new(self.dash.read(
                state,
                livi_core_proto::state::Screen::Main,
                dark,
            ))));
            let wanted = secondary(state);
            if self.secondary.as_ref() != Some(&wanted) {
                self.secondary = Some(wanted.clone());
                out.push(Update::Secondary(wanted.clone()));
            }
            self.others
                .retain(|(screen, _)| wanted.iter().any(|w| w.screen == *screen));
            for w in wanted.iter().filter(|w| w.dashboard) {
                let reader = match self.others.iter_mut().position(|(s, _)| *s == w.screen) {
                    Some(i) => &mut self.others[i].1,
                    None => {
                        self.others.push((w.screen, DashReader::default()));
                        &mut self.others.last_mut().expect("just pushed").1
                    }
                };
                out.push(Update::DashOn(
                    w.screen,
                    Box::new(reader.read(state, w.screen, dark)),
                ));
            }
        }
        if hit("cameras") {
            let found = state["cameras"].as_array().is_some_and(|c| !c.is_empty());
            out.push(Update::Camera(found));
        }
        if livi_ui_model::settings::STATE_READ
            .iter()
            .any(|key| hit(key))
        {
            out.push(Update::Settings);
        }
        if hit("config")
            && let Some(level) = self.volume.update(state)
        {
            out.push(Update::Volume(level));
        }
        if (hit("config") || hit("devices"))
            && let Some(name) = self.phones.update(state)
        {
            out.push(Update::PhoneName(name));
        }
        if keys.is_none()
            && !std::mem::replace(&mut self.started, true)
            && crate::wanted_page().is_none()
        {
            let start = self
                .start
                .clone()
                .unwrap_or_else(|| state["system"]["startPage"].as_str().unwrap_or("/").into());
            out.push(Update::Open(start));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_card_comes_from_the_video_mode_and_the_settings() {
        let state = |active: Value, left: u32| {
            json!({
                "sessions": { "active": active },
                "video": { "projectionCard": true },
                "config": { "videoCardTop": 16, "videoCardBottom": 124, "videoCardLeft": left,
                            "videoCardRight": 20, "videoCardRadius": 22,
                            "projectionWidth": 1920, "projectionHeight": 720 },
            })
        };
        let card = VideoCard::read(&state(json!("carplay"), 252));
        assert!(card.projection && !card.android);
        assert_eq!(
            (card.top, card.bottom, card.left, card.right, card.radius),
            (16, 124, 252, 20, 22)
        );
        // Android Auto gets no card to draw in: full screen, and no button
        let aa = VideoCard::read(&state(json!("androidauto"), 252));
        assert!(!aa.projection && aa.android);
        // a card that does not fit the picture is none, as for the core
        let wide = VideoCard::read(&state(json!("carplay"), 1900));
        assert_eq!((wide.top, wide.left), (0, 0));
        assert_eq!(VideoCard::read(&json!({})), VideoCard::default());
    }

    #[test]
    fn a_new_card_is_handed_on_once() {
        let mut follow = Follow::default();
        let state = json!({ "video": { "projectionCard": true }, "config": { "videoCardTop": 8 } });
        let cards = |updates: Vec<Update>| {
            updates
                .into_iter()
                .filter(|u| matches!(u, Update::VideoCard(_)))
                .count()
        };
        assert_eq!(cards(follow.changed(None, &state)), 1);
        assert_eq!(cards(follow.changed(None, &state)), 0);
    }
}
