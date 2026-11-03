use super::*;

fn zeros(width: usize, height: usize) -> Vec<u8> {
    vec![0; width * height * 4]
}

fn paint(pixels: &mut [u8], width: usize, [x, y, w, h]: [usize; 4], rgba: [u8; 4]) {
    for row in y..y + h {
        for column in x..x + w {
            pixels[(row * width + column) * 4..][..4].copy_from_slice(&rgba);
        }
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

fn pixels(rects: &[Rectangle<i32, Logical>]) -> i64 {
    rects
        .iter()
        .map(|r| i64::from(r.size.w) * i64::from(r.size.h))
        .sum()
}

/// The CarPlay card of the T100 as the compositor keeps what the UI reports: the middle, and the narrow parts beside
/// the round corners
fn card() -> Vec<Rectangle<i32, Logical>> {
    vec![
        rect(316, 18, 2016, 738),
        rect(290, 44, 26, 686),
        rect(2332, 44, 26, 686),
    ]
}

const WINDOW: (i32, i32) = (2400, 900);

#[test]
fn a_new_window_draws_whole_as_without_the_lever() {
    assert!(Hole::default().strips((0, 0).into()).is_none());
}

#[test]
fn without_a_region_or_without_a_video_nothing_is_left_out() {
    let video = Some(rect(0, 0, 2400, 900));
    assert!(holes_in(&[], WINDOW.into(), video).is_empty());
    assert!(holes_in(&card(), WINDOW.into(), None).is_empty());
}

#[test]
fn only_what_lies_over_the_video_and_in_the_window_is_left_out() {
    // the video of the MMI page reaches beyond the window at the top
    let video = Some(rect(982, -43, 1406, 791));
    let holes = holes_in(&[rect(1000, -10, 1400, 800)], WINDOW.into(), video);
    assert_eq!(holes, vec![rect(1000, 0, 1388, 748)]);
}

#[test]
fn parts_too_small_to_pay_stay_drawn() {
    let video = Some(rect(0, 0, 2400, 900));
    // the parts beside the round corners are too narrow
    assert_eq!(
        holes_in(&card(), WINDOW.into(), video),
        vec![rect(316, 18, 2016, 738)]
    );
    // all of it too small
    assert!(holes_in(&[rect(0, 0, 200, 200)], WINDOW.into(), video).is_empty());
}

#[test]
fn the_strips_and_the_holes_are_the_window_without_overlap() {
    let at = Point::<i32, Physical>::from((7, 32));
    // the card without the button on it
    let transparent = Rectangle::subtract_rects_many(card(), [rect(2236, 36, 76, 76)]);
    let holes = holes_in(&transparent, WINDOW.into(), Some(rect(0, 0, 2400, 900)));
    assert!(holes.len() >= 2, "{holes:?}");
    let strips = around(at, WINDOW.into(), &holes).unwrap();
    let holes: Vec<Rectangle<i32, Physical>> = holes
        .iter()
        .map(|h| Rectangle::new((at.x + h.loc.x, at.y + h.loc.y).into(), (h.size.w, h.size.h).into()))
        .collect();
    let area = |r: &Rectangle<i32, Physical>| i64::from(r.size.w) * i64::from(r.size.h);
    assert_eq!(
        strips.iter().chain(&holes).map(area).sum::<i64>(),
        2400 * 900
    );
    for (i, a) in strips.iter().enumerate() {
        assert!(holes.iter().all(|h| a.intersection(*h).is_none()));
        assert!(a.loc.x >= at.x && a.loc.y >= at.y);
        assert!(a.loc.x + a.size.w <= at.x + 2400 && a.loc.y + a.size.h <= at.y + 900);
        for b in &strips[i + 1..] {
            assert!(a.intersection(*b).is_none());
        }
    }
}

#[test]
fn a_hole_in_too_many_pieces_draws_the_window_whole() {
    // a chessboard of holes
    let holes: Vec<_> = (0..6)
        .flat_map(|i| (0..3).map(move |j| rect(100 + i * 300, 100 + j * 250, 128, 128)))
        .collect();
    assert!(pixels(&holes) >= MIN_PIXELS);
    assert!(around((0, 0).into(), WINDOW.into(), &holes).is_none());
}

/// With a real renderer on a software GPU, through Smithay's tracker like on the device: two buffers in turn, an output
/// that is turned. Run with --include-ignored; needs an EGL device, Mesa's software one will do.
mod gl {
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::texture::TextureRenderElement;
    use smithay::backend::renderer::element::utils::CropRenderElement;
    use smithay::backend::renderer::element::{Element, Id, Kind as ElementKind, RenderElement};
    use smithay::backend::renderer::gles::{GlesError, GlesRenderer, GlesTexture};
    use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
    use smithay::backend::renderer::{
        Bind, Color32F, ExportMem, FullDamage, ImportMem, Offscreen, Renderer, RendererSuper,
    };
    use smithay::utils::{Buffer as BufferCoords, Scale, Transform};

    use super::super::*;
    use super::{paint, rect, zeros};

    const WIDTH: i32 = 800;
    const HEIGHT: i32 = 480;
    const CARD: [usize; 4] = [120, 40, 600, 380];
    const RADIUS: usize = 24;
    const BUTTON: [usize; 4] = [660, 60, 40, 40];
    const BACKDROP: [u8; 4] = [20, 30, 40, 255];
    const MAGENTA: Color32F = Color32F::new(1.0, 0.0, 1.0, 1.0);

    smithay::backend::renderer::element::render_elements! {
        Test<=GlesRenderer>;
        Whole=TextureRenderElement<GlesTexture>,
        Cropped=CropRenderElement<TextureRenderElement<GlesTexture>>,
        Hollow=Hollow,
    }

    /// Says it covers the output and draws nothing: what a frame that leaves pixels out looks like
    struct Hollow {
        id: Id,
    }

    impl Element for Hollow {
        fn id(&self) -> &Id {
            &self.id
        }
        fn current_commit(&self) -> CommitCounter {
            CommitCounter::default()
        }
        fn src(&self) -> Rectangle<f64, BufferCoords> {
            Rectangle::from_size((f64::from(WIDTH), f64::from(HEIGHT)).into())
        }
        fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> {
            Rectangle::from_size((WIDTH, HEIGHT).into())
        }
        fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
            OpaqueRegions::from_slice(&[self.geometry(scale)])
        }
    }

    impl RenderElement<GlesRenderer> for Hollow {
        fn draw(
            &self,
            _: &mut <GlesRenderer as RendererSuper>::Frame<'_, '_>,
            _: Rectangle<f64, BufferCoords>,
            _: Rectangle<i32, Physical>,
            _: &[Rectangle<i32, Physical>],
            _: &[Rectangle<i32, Physical>],
        ) -> Result<(), GlesError> {
            Ok(())
        }
    }

    fn renderer() -> GlesRenderer {
        let device = EGLDevice::enumerate()
            .expect("EGL devices")
            .find(EGLDevice::is_software)
            .expect("a software EGL device");
        let display = unsafe { EGLDisplay::new(device) }.expect("EGL display");
        let context = EGLContext::new(&display).expect("EGL context");
        unsafe { GlesRenderer::new(context) }.expect("GLES renderer")
    }

    /// The panel's two buffers in turn, as the compositor's swapchain uses them. The output stands on its side like the
    /// panel, the frames are drawn in the turned picture.
    struct Chain {
        tracker: OutputDamageTracker,
        buffers: [GlesTexture; 2],
        next: usize,
        used: [bool; 2],
    }

    impl Chain {
        fn new(renderer: &mut GlesRenderer, how: FullDamage) -> Self {
            let mut buffer = || {
                Offscreen::<GlesTexture>::create_buffer(
                    renderer,
                    Fourcc::Abgr8888,
                    (HEIGHT, WIDTH).into(),
                )
                .expect("a target")
            };
            let buffers = [buffer(), buffer()];
            let mut tracker = OutputDamageTracker::new((HEIGHT, WIDTH), 1.0, Transform::_90);
            assert_eq!(tracker.full_damage(), FullDamage::Keep);
            tracker.set_full_damage(how);
            Self {
                tracker,
                buffers,
                next: 0,
                used: [false; 2],
            }
        }

        fn frame(&mut self, renderer: &mut GlesRenderer, elements: &[Test]) -> Vec<u8> {
            let i = self.next;
            self.next = 1 - i;
            let age = if self.used[i] { 2 } else { 0 };
            self.used[i] = true;
            let mut target = renderer.bind(&mut self.buffers[i]).expect("bind");
            let clear = Color32F::from(BACKDROP.map(|v| f32::from(v) / 255.0));
            self.tracker
                .render_output(renderer, &mut target, age, elements, clear)
                .expect("render");
            let all = Rectangle::from_size((HEIGHT, WIDTH).into());
            let mapping = renderer
                .copy_framebuffer(&target, all, Fourcc::Abgr8888)
                .expect("copy");
            renderer.map_texture(&mapping).expect("map").to_vec()
        }
    }

    /// LIVI's window over a card: glass colours, alpha 1 around the card, round corners, the card's edge, and on the
    /// card a button when asked for
    fn ui_pixels(button: bool) -> Vec<u8> {
        let (w, h) = (WIDTH as usize, HEIGHT as usize);
        let mut pixels = zeros(w, h);
        for y in 0..h {
            for x in 0..w {
                let rgba = [(x / 4) as u8, (y / 3) as u8, 70, 200];
                pixels[(y * w + x) * 4..][..4].copy_from_slice(&rgba);
            }
        }
        let [cx, cy, cw, ch] = CARD;
        for y in 0..ch {
            for x in 0..cw {
                let near = |at: usize, side: usize| {
                    let far = side - 1 - at;
                    RADIUS.saturating_sub(at.min(far))
                };
                let (dx, dy) = (near(x, cw), near(y, ch));
                if dx * dx + dy * dy <= RADIUS * RADIUS {
                    pixels[((cy + y) * w + cx + x) * 4..][..4].fill(0);
                }
            }
        }
        // the card's edge, half transparent, like a plugin's
        for x in 0..cw {
            pixels[((cy + ch - 1) * w + cx + x) * 4..][..4].copy_from_slice(&[40, 40, 40, 46]);
        }
        if button {
            paint(&mut pixels, w, BUTTON, [0, 0, 0, 140]);
        }
        pixels
    }

    /// What the UI reports for it: the card 2 px inside its edges as a cross without the corners, without the button
    fn reported(button: bool) -> Vec<Rectangle<i32, Logical>> {
        let [cx, cy, cw, ch] = CARD.map(|v| v as i32);
        let (x0, y0, x1, y1) = (cx + 2, cy + 2, cx + cw - 2, cy + ch - 2);
        let r = RADIUS as i32;
        let cross = [
            rect(x0 + r, y0, x1 - x0 - 2 * r, y1 - y0),
            rect(x0, y0 + r, x1 - x0, y1 - y0 - 2 * r),
        ];
        let [bx, by, bw, bh] = BUTTON.map(|v| v as i32);
        let taken = if button {
            vec![rect(bx - 1, by - 1, bw + 2, bh + 2)]
        } else {
            vec![]
        };
        let mut out: Vec<Rectangle<i32, Logical>> = Vec::new();
        for part in cross {
            let new = part.subtract_rects(out.iter().copied());
            out.extend(new);
        }
        Rectangle::subtract_rects_many(out, taken)
    }

    fn texture(renderer: &mut GlesRenderer, pixels: &[u8], size: (i32, i32)) -> GlesTexture {
        renderer
            .import_memory(pixels, Fourcc::Abgr8888, size.into(), false)
            .expect("texture")
    }

    fn element(
        id: &Id,
        context: &GlesRenderer,
        texture: &GlesTexture,
        at: (f64, f64),
    ) -> TextureRenderElement<GlesTexture> {
        TextureRenderElement::from_static_texture(
            id.clone(),
            context.context_id(),
            at,
            texture.clone(),
            1,
            Transform::Normal,
            None,
            None,
            None,
            None,
            ElementKind::Unspecified,
        )
    }

    /// One of two video pictures, opaque, larger than the card as on the MMI page
    fn video_pixels(frame: u8) -> Vec<u8> {
        let (w, h) = (680, 420);
        let mut pixels = zeros(w, h);
        for y in 0..h {
            for x in 0..w {
                let rgba = [
                    (x as u8).wrapping_add(frame),
                    (y as u8).wrapping_mul(3),
                    frame,
                    255,
                ];
                pixels[(y * w + x) * 4..][..4].copy_from_slice(&rgba);
            }
        }
        pixels
    }

    const VIDEO_AT: (f64, f64) = (100.0, 20.0);

    fn video_area() -> Option<Rectangle<i32, Logical>> {
        Some(rect(100, 20, 680, 420))
    }

    /// The UI as the compositor draws it with what the UI reported
    fn with_holes(
        renderer: &GlesRenderer,
        id: &Id,
        ui: &GlesTexture,
        transparent: &[Rectangle<i32, Logical>],
    ) -> Vec<Test> {
        let holes = holes_in(transparent, (WIDTH, HEIGHT).into(), video_area());
        assert!(!holes.is_empty());
        around((0, 0).into(), (WIDTH, HEIGHT).into(), &holes)
            .expect("strips")
            .into_iter()
            .filter_map(|strip| {
                CropRenderElement::from_element(element(id, renderer, ui, (0.0, 0.0)), 1.0, strip)
            })
            .map(Test::Cropped)
            .collect()
    }

    #[test]
    fn what_the_ui_reports_is_all_zero_in_its_pixels() {
        for button in [false, true] {
            let pixels = ui_pixels(button);
            for r in reported(button) {
                for y in r.loc.y..r.loc.y + r.size.h {
                    for x in r.loc.x..r.loc.x + r.size.w {
                        let at = (y * WIDTH + x) as usize * 4;
                        assert_eq!(pixels[at..at + 4], [0, 0, 0, 0], "{x},{y} button {button}");
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "needs an EGL device"]
    fn with_and_without_the_hole_the_picture_is_the_same() {
        let mut renderer = renderer();
        let ui_id = Id::new();
        let ui = texture(&mut renderer, &ui_pixels(true), (WIDTH, HEIGHT));
        let videos = [0, 90, 200].map(|frame| {
            (
                Id::new(),
                texture(&mut renderer, &video_pixels(frame), (680, 420)),
            )
        });
        let mut plain = Chain::new(&mut renderer, FullDamage::Keep);
        let mut holed = Chain::new(&mut renderer, FullDamage::Keep);
        for frame in 0..12 {
            let (id, video) = &videos[frame % videos.len()];
            let whole = vec![
                Test::Whole(element(&ui_id, &renderer, &ui, (0.0, 0.0))),
                Test::Whole(element(id, &renderer, video, VIDEO_AT)),
            ];
            let mut cut = with_holes(&renderer, &ui_id, &ui, &reported(true));
            cut.push(Test::Whole(element(id, &renderer, video, VIDEO_AT)));
            assert!(
                plain.frame(&mut renderer, &whole) == holed.frame(&mut renderer, &cut),
                "frame {frame}"
            );
        }
    }

    /// The region changes with the commits, as when a button or a hint comes and goes: each commit brings a new buffer
    /// and its own region at once
    #[test]
    #[ignore = "needs an EGL device"]
    fn a_hole_that_changes_with_each_commit_keeps_the_picture() {
        let mut renderer = renderer();
        let uis = [false, true].map(|button| {
            (
                button,
                texture(&mut renderer, &ui_pixels(button), (WIDTH, HEIGHT)),
            )
        });
        let videos = [0, 90, 200].map(|frame| {
            (
                Id::new(),
                texture(&mut renderer, &video_pixels(frame), (680, 420)),
            )
        });
        let mut plain = Chain::new(&mut renderer, FullDamage::Keep);
        let mut holed = Chain::new(&mut renderer, FullDamage::Keep);
        // a commit every few frames, at uneven distances so that both buffers see each change
        let commits = [0, 3, 4, 9, 11, 16];
        let mut ui_id = Id::new();
        let mut shown = 0;
        for frame in 0..20 {
            if commits.contains(&frame) {
                // a commit damages the whole window, as Slint's do: a new element
                ui_id = Id::new();
                shown = (frame / 2) % 2;
            }
            let (button, ui) = &uis[shown];
            let (id, video) = &videos[frame % videos.len()];
            let whole = vec![
                Test::Whole(element(&ui_id, &renderer, ui, (0.0, 0.0))),
                Test::Whole(element(id, &renderer, video, VIDEO_AT)),
            ];
            let mut cut = with_holes(&renderer, &ui_id, ui, &reported(*button));
            cut.push(Test::Whole(element(id, &renderer, video, VIDEO_AT)));
            assert!(
                plain.frame(&mut renderer, &whole) == holed.frame(&mut renderer, &cut),
                "frame {frame}, button {button}"
            );
        }
    }

    #[test]
    #[ignore = "needs an EGL device"]
    fn a_region_over_the_button_would_show_this_is_how_the_test_can_fail() {
        let mut renderer = renderer();
        let ui_id = Id::new();
        let ui = texture(&mut renderer, &ui_pixels(true), (WIDTH, HEIGHT));
        let video_id = Id::new();
        let video = texture(&mut renderer, &video_pixels(0), (680, 420));
        let mut plain = Chain::new(&mut renderer, FullDamage::Keep);
        let mut holed = Chain::new(&mut renderer, FullDamage::Keep);
        let whole = vec![
            Test::Whole(element(&ui_id, &renderer, &ui, (0.0, 0.0))),
            Test::Whole(element(&video_id, &renderer, &video, VIDEO_AT)),
        ];
        // the region without the button, the buffer with it
        let mut cut = with_holes(&renderer, &ui_id, &ui, &reported(false));
        cut.push(Test::Whole(element(&video_id, &renderer, &video, VIDEO_AT)));
        assert!(plain.frame(&mut renderer, &whole) != holed.frame(&mut renderer, &cut));
    }

    /// Full frames at the start and when the UI changes, a small video in between: the same as drawn without the lever,
    /// with each of its settings
    #[test]
    #[ignore = "needs an EGL device"]
    fn the_picture_does_not_depend_on_what_a_full_frame_does_first() {
        let mut renderer = renderer();
        let ui_ids = [Id::new(), Id::new()];
        let ui = texture(&mut renderer, &ui_pixels(true), (WIDTH, HEIGHT));
        let videos = [0, 90, 200].map(|frame| {
            (
                Id::new(),
                texture(&mut renderer, &video_pixels(frame), (680, 420)),
            )
        });
        for how in [FullDamage::Discard, FullDamage::Clear(MAGENTA)] {
            let mut kept = Chain::new(&mut renderer, FullDamage::Keep);
            let mut lever = Chain::new(&mut renderer, how);
            for frame in 0..16 {
                // a new UI element damages the whole window, as a commit of Slint does
                let ui_id = &ui_ids[usize::from(frame % 8 >= 4)];
                let (id, video) = &videos[frame % videos.len()];
                let elements = |renderer: &GlesRenderer| {
                    vec![
                        Test::Whole(element(ui_id, renderer, &ui, (0.0, 0.0))),
                        Test::Whole(element(id, renderer, video, VIDEO_AT)),
                    ]
                };
                let one = elements(&renderer);
                let two = elements(&renderer);
                assert!(
                    kept.frame(&mut renderer, &one) == lever.frame(&mut renderer, &two),
                    "{how:?} frame {frame}"
                );
            }
        }
    }

    #[test]
    #[ignore = "needs an EGL device"]
    fn a_full_frame_that_leaves_pixels_out_shows_magenta_this_is_how_the_test_can_fail() {
        let mut renderer = renderer();
        let hollow = [Test::Hollow(Hollow { id: Id::new() })];
        let mut chain = Chain::new(&mut renderer, FullDamage::Clear(MAGENTA));
        let pixels = chain.frame(&mut renderer, &hollow);
        assert_eq!(pixels[..4], [255, 0, 255, 255]);
        let mut chain = Chain::new(&mut renderer, FullDamage::Keep);
        let pixels = chain.frame(&mut renderer, &hollow);
        assert_ne!(pixels[..4], [255, 0, 255, 255]);
        let _ = Color32F::from(BACKDROP.map(|v| f32::from(v) / 255.0)).r();
    }
}
