use serde::{Deserialize, Serialize};

use crate::state::Screen;

/// Trial: whether the phone's cluster picture takes touches and a knob (`LIVI_CLUSTER_TOUCH=1`). The core announces the
/// devices and forwards to them, the UI sends what lands on the map.
pub fn cluster_touch() -> bool {
    std::env::var("LIVI_CLUSTER_TOUCH").is_ok_and(|v| v == "1")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Input {
    Pointer {
        screen: Screen,
        points: Vec<Point>,
    },
    Key {
        code: String,
        down: bool,
    },
    /// The knob of the cluster picture: `wheel` is a turn by that many detents, which rests again by itself,
    /// `select` its button, held until the next one says otherwise
    ClusterKnob {
        wheel: f64,
        select: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub phase: Phase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Down,
    Move,
    Up,
    Cancel,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn pointer_on_the_wire() {
        let input = Input::Pointer {
            screen: Screen::Main,
            points: vec![Point {
                id: 0,
                x: 0.25,
                y: 0.5,
                phase: Phase::Down,
            }],
        };
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            json!({ "kind": "pointer", "screen": "main", "points": [{ "id": 0, "x": 0.25, "y": 0.5, "phase": "down" }] })
        );
    }

    #[test]
    fn the_cluster_knob_on_the_wire() {
        let input = Input::ClusterKnob {
            wheel: -1.0,
            select: false,
        };
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            json!({ "kind": "clusterKnob", "wheel": -1.0, "select": false })
        );
    }

    #[test]
    fn key_on_the_wire() {
        let input: Input =
            serde_json::from_value(json!({ "kind": "key", "code": "KeyH", "down": true })).unwrap();
        assert_eq!(
            input,
            Input::Key {
                code: "KeyH".into(),
                down: true
            }
        );
    }
}
