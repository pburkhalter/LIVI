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

#[test]
fn an_empty_buffer_is_clear_up_to_its_last_pixel() {
    // neither side is a multiple of a square
    assert_eq!(
        largest_clear(&zeros(301, 299), 301, 299),
        Some([0, 0, 301, 299])
    );
}

#[test]
fn a_full_buffer_has_no_hole_and_a_short_one_is_not_read() {
    let mut pixels = zeros(400, 300);
    paint(&mut pixels, 400, [0, 0, 400, 300], [9, 9, 9, 255]);
    assert_eq!(largest_clear(&pixels, 400, 300), None);
    assert_eq!(largest_clear(&pixels[..16], 400, 300), None);
    assert_eq!(largest_clear(&[], 0, 0), None);
}

#[test]
fn a_hole_too_small_to_pay_is_none() {
    let mut pixels = zeros(400, 300);
    paint(&mut pixels, 400, [0, 0, 400, 300], [9, 9, 9, 255]);
    paint(&mut pixels, 400, [16, 16, 248, 248], [0, 0, 0, 0]);
    assert_eq!(largest_clear(&pixels, 400, 300), None);
    paint(&mut pixels, 400, [16, 16, 256, 256], [0, 0, 0, 0]);
    assert_eq!(largest_clear(&pixels, 400, 300), Some([16, 16, 256, 256]));
}

#[test]
fn only_a_pixel_of_zero_counts() {
    for rgba in [[0, 0, 0, 1], [1, 0, 0, 0], [0, 0, 1, 0]] {
        let mut pixels = zeros(600, 300);
        paint(&mut pixels, 600, [300, 0, 1, 300], rgba);
        let found = largest_clear(&pixels, 600, 300).unwrap();
        assert!(found[0] + found[2] <= 300 || found[0] > 300, "{found:?}");
    }
}

#[test]
fn a_card_with_a_ring_loses_the_ring_and_the_square_it_touches() {
    // alpha 1 of 255 around a card, as LIVI draws it, and one thin line on the card's edge
    let mut pixels = zeros(800, 600);
    paint(&mut pixels, 800, [0, 0, 800, 600], [0, 0, 0, 1]);
    paint(&mut pixels, 800, [100, 80, 600, 440], [0, 0, 0, 0]);
    let [x, y, w, h] = largest_clear(&pixels, 800, 600).unwrap();
    assert_eq!([x, y, w, h], [104, 80, 592, 440]);
}

#[test]
fn something_drawn_on_the_card_stays_outside_the_hole() {
    let mut pixels = zeros(800, 480);
    paint(&mut pixels, 800, [700, 20, 72, 72], [0, 0, 0, 140]);
    let [x, y, w, h] = largest_clear(&pixels, 800, 480).unwrap();
    assert_eq!([x, y, w, h], [0, 0, 696, 480]);
}

/// A generator with no crate: the same run every time
fn next(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}

#[test]
fn what_is_found_is_all_zero_and_nothing_larger_exists() {
    const SIDE: usize = 512;
    let tiles = SIDE / TILE;
    let mut seed = 0x1234_5678;
    for _ in 0..30 {
        let mut pixels = zeros(SIDE, SIDE);
        for _ in 0..1 + next(&mut seed) % 5 {
            let x = next(&mut seed) as usize % SIDE;
            let y = next(&mut seed) as usize % SIDE;
            let w = (1 + next(&mut seed) as usize % 40).min(SIDE - x);
            let h = (1 + next(&mut seed) as usize % 40).min(SIDE - y);
            paint(&mut pixels, SIDE, [x, y, w, h], [0, 0, 0, 1]);
        }
        let Some([x, y, w, h]) = largest_clear(&pixels, SIDE, SIDE) else {
            continue;
        };
        for row in y..y + h {
            assert!(
                pixels[(row * SIDE + x) * 4..][..w * 4]
                    .iter()
                    .all(|b| *b == 0)
            );
        }
        // every rectangle of squares, by a sum over the squares that are not clear
        let mut sum = vec![0_u32; (tiles + 1) * (tiles + 1)];
        for ty in 0..tiles {
            for tx in 0..tiles {
                let mut any = 0;
                for row in ty * TILE..(ty + 1) * TILE {
                    any |= pixels[(row * SIDE + tx * TILE) * 4..][..TILE * 4]
                        .iter()
                        .fold(0, |a, b| a | b);
                }
                sum[(ty + 1) * (tiles + 1) + tx + 1] = u32::from(any != 0)
                    + sum[ty * (tiles + 1) + tx + 1]
                    + sum[(ty + 1) * (tiles + 1) + tx]
                    - sum[ty * (tiles + 1) + tx];
            }
        }
        let at = |x: usize, y: usize| sum[y * (tiles + 1) + x];
        let mut largest = 0;
        for y0 in 0..tiles {
            for y1 in y0 + 1..=tiles {
                for x0 in 0..tiles {
                    for x1 in x0 + 1..=tiles {
                        if at(x1, y1) + at(x0, y0) == at(x0, y1) + at(x1, y0) {
                            largest = largest.max((x1 - x0) * (y1 - y0));
                        }
                    }
                }
            }
        }
        assert_eq!(w * h, largest * TILE * TILE);
    }
}

fn commit(n: usize) -> CommitCounter {
    let mut counter = CommitCounter::default();
    for _ in 0..n {
        counter.increment();
    }
    counter
}

fn cut() -> Option<Cut> {
    Some(Cut {
        window: (2400, 900).into(),
        hole: Rectangle::new((1000, 24).into(), (1300, 720).into()),
    })
}

#[test]
fn a_new_window_draws_whole_as_without_the_lever() {
    assert!(Hole::default().strips((0, 0).into()).is_none());
}

fn area(w: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((982, 0).into(), (w, 748).into())
}

#[test]
fn a_buffer_is_read_once_when_it_has_stood() {
    let start = Instant::now();
    let mut hole = Hole::default();
    assert!(!hole.due(commit(1), area(1400), start), "just committed");
    assert!(!hole.due(commit(1), area(1400), start + SETTLE / 2));
    assert!(hole.due(commit(1), area(1400), start + SETTLE));
    hole.read = Some(area(1400));
    assert!(!hole.due(commit(1), area(1400), start + SETTLE * 2));
}

#[test]
fn a_video_that_grows_is_read_again_and_the_hole_stays_until_then() {
    let start = Instant::now();
    let mut hole = Hole::default();
    hole.due(commit(1), area(640), start);
    hole.read = Some(area(640));
    hole.cut = cut();
    assert!(hole.due(commit(1), area(1400), start + SETTLE));
    assert!(hole.strips((0, 0).into()).is_some());
}

#[test]
fn a_commit_ends_the_hole_before_its_frame() {
    let start = Instant::now();
    let mut hole = Hole::default();
    hole.due(commit(1), area(1400), start);
    hole.read = Some(area(1400));
    hole.cut = cut();
    assert!(hole.strips((0, 0).into()).is_some());
    assert!(
        !hole.due(commit(2), area(1400), start + SETTLE * 9),
        "the new buffer stands from now on"
    );
    assert!(hole.strips((0, 0).into()).is_none());
    assert!(hole.due(commit(2), area(1400), start + SETTLE * 10));
}

#[test]
fn a_commit_ends_the_hole_also_when_no_video_shows() {
    let start = Instant::now();
    let mut hole = Hole::default();
    hole.note(commit(1), start);
    hole.read = Some(area(1400));
    hole.cut = cut();
    assert!(hole.note(commit(2), start + SETTLE));
    assert!(hole.strips((0, 0).into()).is_none());
}

#[test]
fn the_strips_and_the_hole_are_the_window_without_overlap() {
    let at = Point::<i32, Physical>::from((7, 32));
    let Cut { window, hole } = cut().unwrap();
    let strips = around(at, window, hole);
    assert!(strips.len() <= 4);
    let hole = Rectangle::<i32, Physical>::new(
        (at.x + hole.loc.x, at.y + hole.loc.y).into(),
        (hole.size.w, hole.size.h).into(),
    );
    let area = |r: &Rectangle<i32, Physical>| i64::from(r.size.w) * i64::from(r.size.h);
    assert_eq!(
        strips.iter().map(area).sum::<i64>() + area(&hole),
        2400 * 900
    );
    for (i, a) in strips.iter().enumerate() {
        assert!(a.intersection(hole).is_none());
        assert!(a.loc.x >= at.x && a.loc.y >= at.y);
        assert!(a.loc.x + a.size.w <= at.x + 2400 && a.loc.y + a.size.h <= at.y + 900);
        for b in &strips[i + 1..] {
            assert!(a.intersection(*b).is_none());
        }
    }
}

/// With a real renderer on a software GPU, through Smithay's tracker like on the device: two buffers in turn, an output
/// that is turned. Run with --include-ignored; needs an EGL device, Mesa's software one will do.
mod gl {
    use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::texture::TextureRenderElement;
    use smithay::backend::renderer::element::utils::CropRenderElement;
    use smithay::backend::renderer::element::{Element, Id, Kind as ElementKind, RenderElement};
    use smithay::backend::renderer::gles::ffi;
    use smithay::backend::renderer::utils::OpaqueRegions;
    use smithay::backend::renderer::{
        Bind, Color32F, ExportMem, FullDamage, ImportMem, Offscreen, Renderer, RendererSuper,
    };
    use smithay::utils::Scale;

    use super::super::*;
    use super::{paint, zeros};

    const WIDTH: i32 = 800;
    const HEIGHT: i32 = 480;
    const CARD: [usize; 4] = [120, 40, 600, 380];
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

    /// LIVI's window over a card: glass colours, alpha 1 around the card, round corners, a button on the card
    fn ui_pixels() -> Vec<u8> {
        let (w, h) = (WIDTH as usize, HEIGHT as usize);
        let mut pixels = zeros(w, h);
        for y in 0..h {
            for x in 0..w {
                let rgba = [(x / 4) as u8, (y / 3) as u8, 70, 200];
                pixels[(y * w + x) * 4..][..4].copy_from_slice(&rgba);
            }
        }
        let [cx, cy, cw, ch] = CARD;
        const RADIUS: usize = 24;
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
        paint(&mut pixels, w, BUTTON, [0, 0, 0, 140]);
        pixels
    }

    fn texture(
        renderer: &mut GlesRenderer,
        pixels: &[u8],
        size: (i32, i32),
        flipped: bool,
    ) -> GlesTexture {
        renderer
            .import_memory(pixels, Fourcc::Abgr8888, size.into(), flipped)
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

    fn with_hole(
        renderer: &GlesRenderer,
        id: &Id,
        ui: &GlesTexture,
        hole: Rectangle<i32, Logical>,
    ) -> Vec<Test> {
        around((0, 0).into(), (WIDTH, HEIGHT).into(), hole)
            .into_iter()
            .filter_map(|strip| {
                CropRenderElement::from_element(element(id, renderer, ui, (0.0, 0.0)), 1.0, strip)
            })
            .map(Test::Cropped)
            .collect()
    }

    #[test]
    #[ignore = "needs an EGL device"]
    fn the_hole_reads_what_the_window_shows() {
        let mut renderer = renderer();
        let pixels = ui_pixels();
        let area = Rectangle::new((100, 20).into(), (680, 420).into());
        for flipped in [false, true] {
            let mut stored = pixels.clone();
            if flipped {
                let row = WIDTH as usize * 4;
                stored = pixels.chunks_exact(row).rev().flatten().copied().collect();
            }
            let ui = texture(&mut renderer, &stored, (WIDTH, HEIGHT), flipped);
            assert_eq!(ui.is_y_inverted(), flipped);
            // as an imported dma-buf is: Smithay draws an inverted texture at negative coordinates, a texture from memory
            // would stop at its edge
            renderer
                .with_context(|gl| unsafe {
                    gl.BindTexture(ffi::TEXTURE_2D, ui.tex_id());
                    gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::REPEAT as i32);
                    gl.BindTexture(ffi::TEXTURE_2D, 0);
                })
                .unwrap();
            let direct = largest_clear_in(&mut renderer, &ui, area).unwrap().unwrap();
            let copied = read_a_copy(&mut renderer, &ui, area).unwrap().unwrap();
            for hole in [direct, copied] {
                // inside the card, beside the button, above its lower edge; the rows below are looked up in the window's
                // picture, so that a buffer drawn upside down must come out at the same place
                assert_eq!((hole.loc.x, hole.size.w), (132, 528), "{hole:?}");
                assert!(hole.size.h >= 360, "{hole:?}");
                for y in hole.loc.y..hole.loc.y + hole.size.h {
                    let at = (y * WIDTH + hole.loc.x) as usize * 4;
                    let row = &pixels[at..at + hole.size.w as usize * 4];
                    assert!(row.iter().all(|b| *b == 0), "row {y}");
                }
            }
        }
    }

    #[test]
    #[ignore = "needs an EGL device"]
    fn with_and_without_the_hole_the_picture_is_the_same() {
        let mut renderer = renderer();
        let ui_id = Id::new();
        let ui = texture(&mut renderer, &ui_pixels(), (WIDTH, HEIGHT), false);
        let area = Rectangle::new((100, 20).into(), (680, 420).into());
        let hole = largest_clear_in(&mut renderer, &ui, area).unwrap().unwrap();
        let videos = [0, 90, 200].map(|frame| {
            (
                Id::new(),
                texture(&mut renderer, &video_pixels(frame), (680, 420), false),
            )
        });
        let mut plain = Chain::new(&mut renderer, FullDamage::Keep);
        let mut holed = Chain::new(&mut renderer, FullDamage::Keep);
        for frame in 0..12 {
            let (id, video) = &videos[frame % videos.len()];
            let video = Test::Whole(element(id, &renderer, video, (100.0, 20.0)));
            let whole = vec![
                Test::Whole(element(&ui_id, &renderer, &ui, (0.0, 0.0))),
                Test::Whole(element(
                    id,
                    &renderer,
                    &videos[frame % videos.len()].1,
                    (100.0, 20.0),
                )),
            ];
            let mut cut = with_hole(&renderer, &ui_id, &ui, hole);
            assert!(!cut.is_empty());
            cut.push(video);
            assert!(
                plain.frame(&mut renderer, &whole) == holed.frame(&mut renderer, &cut),
                "frame {frame}"
            );
        }
    }

    #[test]
    #[ignore = "needs an EGL device"]
    fn a_hole_that_is_too_large_would_show_this_is_how_the_test_can_fail() {
        let mut renderer = renderer();
        let ui_id = Id::new();
        let ui = texture(&mut renderer, &ui_pixels(), (WIDTH, HEIGHT), false);
        let video_id = Id::new();
        let video = texture(&mut renderer, &video_pixels(0), (680, 420), false);
        // over the button
        let hole = Rectangle::new((140, 50).into(), (560, 360).into());
        let mut plain = Chain::new(&mut renderer, FullDamage::Keep);
        let mut holed = Chain::new(&mut renderer, FullDamage::Keep);
        let whole = vec![
            Test::Whole(element(&ui_id, &renderer, &ui, (0.0, 0.0))),
            Test::Whole(element(&video_id, &renderer, &video, (100.0, 20.0))),
        ];
        let mut cut = with_hole(&renderer, &ui_id, &ui, hole);
        cut.push(Test::Whole(element(
            &video_id,
            &renderer,
            &video,
            (100.0, 20.0),
        )));
        assert!(plain.frame(&mut renderer, &whole) != holed.frame(&mut renderer, &cut));
    }

    /// Full frames at the start and when the UI changes, a small video in between: the same as drawn without the lever,
    /// with each of its settings
    #[test]
    #[ignore = "needs an EGL device"]
    fn the_picture_does_not_depend_on_what_a_full_frame_does_first() {
        let mut renderer = renderer();
        let ui_ids = [Id::new(), Id::new()];
        let ui = texture(&mut renderer, &ui_pixels(), (WIDTH, HEIGHT), false);
        let videos = [0, 90, 200].map(|frame| {
            (
                Id::new(),
                texture(&mut renderer, &video_pixels(frame), (680, 420), false),
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
                        Test::Whole(element(id, renderer, video, (100.0, 20.0))),
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
