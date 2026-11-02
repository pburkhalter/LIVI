use super::{
    Offered, assign, camera_ready, choose, panel_name, size_of, splash_over, wl_output_in,
};

fn mode(w: u16, h: u16, refresh: u32, preferred: bool) -> Offered {
    Offered {
        size: (w, h),
        refresh,
        preferred,
    }
}

#[test]
fn the_setting_wins_with_its_fastest_refresh() {
    let modes = [
        mode(1920, 1080, 60, true),
        mode(1280, 720, 50, false),
        mode(1280, 720, 60, false),
    ];
    assert_eq!(choose(&modes, Some((1280, 720))), Some(2));
    assert_eq!(size_of("1024x600"), Some((1024, 600)));
    assert_eq!(size_of("auto"), None);
}

#[test]
fn without_a_setting_the_panel_stays_at_most_hd() {
    let small_panel = [mode(800, 480, 60, true), mode(640, 480, 60, false)];
    assert_eq!(choose(&small_panel, None), Some(0));
    let xga = [mode(1024, 768, 60, true), mode(800, 600, 60, false)];
    assert_eq!(
        choose(&xga, None),
        Some(0),
        "a 4:3 panel keeps its native mode below the HD pixel count"
    );
    let tv = [
        mode(3840, 2160, 60, true),
        mode(1920, 1080, 60, false),
        mode(1280, 720, 60, false),
    ];
    assert_eq!(
        choose(&tv, None),
        Some(2),
        "a 4k panel drops to 720p like the kiosk did"
    );
    assert_eq!(
        choose(&tv, Some((1366, 768))),
        Some(2),
        "a size the panel lacks falls back"
    );
    let only_big = [mode(2560, 1440, 60, true), mode(1920, 1080, 60, false)];
    assert_eq!(
        choose(&only_big, None),
        Some(1),
        "the smallest when nothing fits"
    );
    assert_eq!(choose(&[], None), None);
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn screens_take_their_connector_or_the_next_free_one() {
    let connected = names(&["HDMI-A-1", "HDMI-A-2", "DSI-1"]);
    let auto = [(0, None), (1, None)];
    assert_eq!(
        assign(&auto, &connected),
        [(0, "HDMI-A-1".into()), (1, "HDMI-A-2".into())]
    );
    let cluster_on_dsi = [(0, None), (1, Some("DSI-1".to_string()))];
    assert_eq!(
        assign(&cluster_on_dsi, &connected),
        [(0, "HDMI-A-1".into()), (1, "DSI-1".into())]
    );
    let main_on_second = [(0, Some("HDMI-A-2".to_string())), (1, None)];
    assert_eq!(
        assign(&main_on_second, &connected),
        [(0, "HDMI-A-2".into()), (1, "HDMI-A-1".into())]
    );
    let gone = [(0, None), (1, Some("HDMI-A-3".to_string()))];
    assert_eq!(
        assign(&gone, &connected),
        [(0, "HDMI-A-1".into())],
        "a chosen connector that is not there leaves the screen dark"
    );
    assert_eq!(
        assign(&auto, &names(&["HDMI-A-1"])),
        [(0, "HDMI-A-1".into())]
    );
    let pinned_cluster = [(0, None), (1, Some("HDMI-A-1".to_string()))];
    assert_eq!(
        assign(&pinned_cluster, &names(&["HDMI-A-1"])),
        [(0, "HDMI-A-1".into())],
        "main keeps the only display even when the cluster asks for it"
    );
    let main_gone = [(0, Some("DSI-1".to_string())), (1, None)];
    assert_eq!(
        assign(&main_gone, &names(&["HDMI-A-1"])),
        [(0, "HDMI-A-1".into())],
        "main never stays dark while a display is connected"
    );
    let cluster_on_first = [(0, None), (1, Some("HDMI-A-1".to_string()))];
    assert_eq!(
        assign(&cluster_on_first, &names(&["HDMI-A-1", "HDMI-A-2"])),
        [(0, "HDMI-A-2".into()), (1, "HDMI-A-1".into())],
        "main steps aside to a display nobody asked for"
    );
}

#[test]
fn a_touch_device_names_its_display_in_the_udev_data() {
    assert_eq!(
        wl_output_in("E:ID_INPUT=1\nE:WL_OUTPUT=DSI-1\nE:ID_INPUT_TOUCHSCREEN=1\n").as_deref(),
        Some("DSI-1")
    );
    assert_eq!(wl_output_in("E:ID_INPUT=1\n"), None);
    assert_eq!(wl_output_in("E:WL_OUTPUT=\n"), None);
}

#[test]
fn the_panel_name_comes_from_the_edid_descriptor() {
    let mut edid = vec![0u8; 128];
    let block = 54 + 18;
    edid[block + 3] = 0xFC;
    edid[block + 5..block + 5 + 13].copy_from_slice(b"Elecrow 8in\n ");
    assert_eq!(panel_name(&edid).as_deref(), Some("Elecrow 8in"));
    assert_eq!(panel_name(&[0u8; 128]), None);
    assert_eq!(panel_name(&[0u8; 20]), None);
}

/// A model of the pacing in `render`, `drawn` and `vblank` for a client with two buffers, in milliseconds. It follows the
/// order of those functions, it does not run them: they need a display.
mod pacing {
    const VBLANK: f64 = 1000.0 / 60.0;

    #[derive(Debug)]
    struct Pace {
        frame_ms: f64,
        commit_to_flip_ms: f64,
    }

    struct Model {
        early: bool,
        cpu: f64,
        gpu: f64,
        now: f64,
        // the compositor, as in `Output`
        drawing_until: Option<f64>,
        frame_pending: bool,
        needs_redraw: bool,
        held: Option<usize>,
        // the client
        attached: Option<usize>,
        drawing_into: usize,
        client_done: Option<f64>,
        wants_callback: bool,
        wants_buffer: bool,
        // when the frame was committed, while it goes through
        committed: Option<f64>,
        rendering: Option<f64>,
        flipping: Option<f64>,
        flips: Vec<f64>,
        commit_to_flip: Vec<f64>,
    }

    impl Model {
        fn new(early: bool, cpu: f64, gpu: f64) -> Self {
            Self {
                early,
                cpu,
                gpu,
                now: 0.0,
                drawing_until: None,
                frame_pending: false,
                needs_redraw: false,
                held: None,
                attached: None,
                drawing_into: 0,
                client_done: Some(cpu),
                wants_callback: false,
                wants_buffer: false,
                committed: None,
                rendering: None,
                flipping: None,
                flips: Vec::new(),
                commit_to_flip: Vec::new(),
            }
        }

        /// `wayland_wait_for_buffer_release` in the WSEGL: a buffer that is neither attached nor held
        fn next_buffer(&mut self) {
            let free = (0..2).find(|b| Some(*b) != self.attached && Some(*b) != self.held);
            self.wants_buffer = free.is_none();
            if let Some(buffer) = free {
                self.drawing_into = buffer;
                self.client_done = Some(self.now + self.cpu);
            }
        }

        fn frame_callback(&mut self) {
            if std::mem::take(&mut self.wants_callback) {
                self.next_buffer();
            }
        }

        fn commit(&mut self) {
            self.client_done = None;
            assert_ne!(
                Some(self.drawing_into),
                self.held,
                "drew into what the GPU reads"
            );
            self.attached = Some(self.drawing_into);
            self.committed = Some(self.now);
            self.needs_redraw = true;
            self.wants_callback = true;
            if !self.frame_pending && self.drawing_until.is_none() {
                self.render();
            }
        }

        fn render(&mut self) {
            self.needs_redraw = false;
            self.drawing_until = Some(self.now + self.gpu);
            self.held = self.attached;
            self.rendering = self.committed.take();
            if self.early {
                self.frame_callback();
            }
        }

        fn drawn(&mut self) {
            self.drawing_until = None;
            self.frame_pending = true;
            self.flipping = self.rendering.take();
            if !self.early {
                self.frame_callback();
            }
        }

        fn vblank(&mut self) {
            if !self.frame_pending {
                return;
            }
            self.frame_pending = false;
            self.held = None;
            self.flips.push(self.now);
            if let Some(commit) = self.flipping.take() {
                self.commit_to_flip.push(self.now - commit);
            }
            if self.wants_buffer {
                self.next_buffer();
            }
            if self.needs_redraw {
                self.render();
            }
        }

        fn run(mut self) -> Pace {
            let mut next_vblank = VBLANK;
            while self.flips.len() < 200 {
                let client = self.client_done.unwrap_or(f64::MAX);
                let gpu = self.drawing_until.unwrap_or(f64::MAX);
                self.now = client.min(gpu).min(next_vblank);
                if self.now == client {
                    self.commit();
                } else if self.now == gpu {
                    self.drawn();
                } else {
                    next_vblank += VBLANK;
                    self.vblank();
                }
            }
            // after the start, when the first frames find everything free
            let flips = &self.flips[50..];
            let waits = &self.commit_to_flip[50..];
            Pace {
                frame_ms: (flips[flips.len() - 1] - flips[0]) / (flips.len() - 1) as f64,
                commit_to_flip_ms: waits.iter().sum::<f64>() / waits.len() as f64,
            }
        }
    }

    fn pace(early: bool, cpu: f64, gpu: f64) -> Pace {
        Model::new(early, cpu, gpu).run()
    }

    #[test]
    fn without_the_lever_cpu_and_gpu_add_up() {
        let pace = pace(false, 15.0, 25.0);
        assert!((pace.frame_ms - 40.0).abs() < 1.0, "{pace:?}");
    }

    #[test]
    fn with_the_lever_they_overlap_and_the_frame_takes_two_vblanks() {
        let pace = pace(true, 15.0, 25.0);
        assert!((pace.frame_ms - VBLANK * 2.0).abs() < 0.5, "{pace:?}");
    }

    #[test]
    fn the_price_is_less_than_one_frame_more_from_commit_to_flip() {
        let (off, on) = (pace(false, 15.0, 25.0), pace(true, 15.0, 25.0));
        println!("off {off:?}, on {on:?}");
        assert!(
            on.commit_to_flip_ms > off.commit_to_flip_ms,
            "{off:?} {on:?}"
        );
        assert!(
            on.commit_to_flip_ms < off.commit_to_flip_ms + on.frame_ms,
            "{off:?} {on:?}"
        );
    }

    #[test]
    fn two_buffers_are_enough_and_no_frame_is_lost_at_any_speed() {
        for cpu in [1.0, 5.0, 15.0, 30.0, 50.0] {
            for gpu in [1.0, 7.0, 15.0, 25.0, 45.0, 90.0] {
                for early in [false, true] {
                    // `commit` panics when the client draws into what the GPU reads
                    let pace = pace(early, cpu, gpu);
                    assert!(
                        pace.frame_ms >= VBLANK - 0.1,
                        "{early} {cpu} {gpu} {pace:?}"
                    );
                    assert!(
                        pace.frame_ms <= cpu + gpu + 2.0 * VBLANK,
                        "{early} {cpu} {gpu} {pace:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_fast_client_keeps_the_panel_rate_either_way() {
        let (off, on) = (pace(false, 2.0, 5.0), pace(true, 2.0, 5.0));
        assert!((off.frame_ms - VBLANK).abs() < 0.5, "{off:?}");
        assert!((on.frame_ms - VBLANK).abs() < 0.5, "{on:?}");
    }
}

#[test]
fn the_start_waits_for_the_ui_unless_the_camera_or_the_time_ends_it() {
    assert_eq!(splash_over(false, false, false), None);
    assert_eq!(
        splash_over(true, false, false),
        Some("first frame of the UI")
    );
    // the UI first when both are there: the log says what the screen shows
    assert_eq!(splash_over(true, true, true), Some("first frame of the UI"));
    assert_eq!(
        splash_over(false, true, false),
        Some("first frame with the camera, before the UI")
    );
    assert_eq!(
        splash_over(false, false, true),
        Some("first frame without the UI (splash wait over)")
    );
}

#[test]
fn the_camera_counts_only_after_a_few_frames() {
    assert!(!camera_ready(0));
    assert!(!camera_ready(1));
    assert!(camera_ready(5));
    assert!(camera_ready(u32::MAX));
}
