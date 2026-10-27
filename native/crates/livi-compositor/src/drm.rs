use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::{DrmCompositor, FrameFlags, PrimaryPlaneElement};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;

use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, DrmNode};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, InputEvent, KeyState, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::Kind as ElementKind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::{Buffer, RendererSurfaceStateUserData};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::input::keyboard::{FilterResult, Keysym};
use smithay::output::OutputModeSource;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{LoopHandle, channel};
use smithay::reexports::drm::control::property::ValueType;
use smithay::reexports::drm::control::{Device as _, Mode, ModeTypeFlags, connector, crtc};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Point, SERIAL_COUNTER, Scale, Size, Transform};
use smithay::wayland::compositor::{TraversalAction, with_states, with_surface_tree_downward};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke};

use crate::render::LiviElement;
use crate::state::{Kind, LiviState};

mod libinput;
mod modes;

use libinput::*;
use modes::*;

type Surface =
    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// A frame for the `gpu-wait` thread: the CRTC it goes to, its number and the point the GPU reaches when done
type GpuWait = std::sync::mpsc::Sender<(crtc::Handle, u64, SyncPoint)>;

pub struct DrmState {
    session: LibSeatSession,
    libinput: Libinput,
    device: DrmDevice,
    gbm: GbmDevice<DrmDeviceFd>,
    formats: FormatSet,
    import_node: Option<DrmNode>,
    loop_handle: LoopHandle<'static, LiviState>,
    // frames whose GPU work is still running go here; a thread waits for them and hands them back to the loop
    gpu_wait: GpuWait,
    // numbers the frames sent to `gpu_wait`, so that one coming back after its output was reset or replaced is dropped
    frames: u64,
    connectors: Vec<Connector>,
    outputs: Vec<Output>,
    wanted: BTreeSet<usize>,
    mode_settings: HashMap<usize, Option<String>>,
    touch_outputs: HashMap<String, String>,
    active: bool,
    mouse_active: bool,
    cursor: MemoryRenderBuffer,
    /// Until the UI has drawn its first frame (at most until this time), the displays keep what they show:
    /// the boot splash that Plymouth leaves with "plymouth quit --retain-splash", instead of the backdrop colour.
    splash_until: Option<Instant>,
    splash_timer: bool,
}

/// Longest wait for the first frame of the UI before the backdrop is shown after all
const SPLASH_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

struct Connector {
    handle: connector::Handle,
    name: String,
    panel: String,
    connected: bool,
    modes: Vec<Mode>,
    panel_mm: Option<(i32, i32)>,
    transform: Transform,
    crtcs: Vec<crtc::Handle>,
}

struct Output {
    screen_idx: usize,
    connector: String,
    crtc: crtc::Handle,
    mode: Mode,
    transform: Transform,
    surface: Surface,
    frame_pending: bool,
    // rendered, waiting for the GPU to finish before it goes to the display: the frame's number
    drawing: Option<u64>,
    direct: usize,
    needs_redraw: bool,
    /// Client buffers the queued frame was rendered from, released on its vblank. Some drivers attach no
    /// implicit fences to dma-bufs (e.g. a V4L2 stateless decoder feeding a PowerVR GPU), so a buffer handed
    /// back to the client right after queue_frame can be overwritten while the GPU still samples it.
    held: Vec<Buffer>,
}

impl Output {
    fn physical(&self) -> (i32, i32) {
        let (w, h) = self.mode.size();
        (i32::from(w), i32::from(h))
    }

    fn logical(&self) -> (i32, i32) {
        let size = self
            .transform
            .transform_size(Size::<i32, smithay::utils::Physical>::from(self.physical()));
        (size.w, size.h)
    }
}

enum KeyAction {
    SwitchVt(i32),
    Quit,
}

pub fn wanted() -> bool {
    match std::env::var("LIVI_COMPOSITOR_BACKEND").as_deref() {
        Ok("drm") => true,
        Ok("wayland") => false,
        _ => std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty()),
    }
}

pub fn init(state: &mut LiviState, handle: &LoopHandle<'static, LiviState>) {
    match start(state, handle) {
        Ok(drm) => {
            state.drm = Some(drm);
            arrange(state);
        }
        Err(e) => {
            log::error!("cannot drive the display directly: {e}");
            state.running = false;
        }
    }
}

fn start(
    state: &mut LiviState,
    handle: &LoopHandle<'static, LiviState>,
) -> Result<DrmState, Box<dyn std::error::Error>> {
    let (mut session, session_events) = LibSeatSession::new()?;
    handle
        .insert_source(session_events, |event, _, state: &mut LiviState| {
            session_event(state, event)
        })
        .map_err(|e| e.error)?;
    let seat = session.seat();
    let (device, notifier, fd) = open_device(&mut session, &seat)?;

    let gbm = GbmDevice::new(fd.clone())?;
    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let renderer = unsafe { GlesRenderer::new(EGLContext::new(&egl_display)?)? };
    let formats = renderer.egl_context().dmabuf_render_formats().clone();
    let scanout = fd.dev_id().ok().map(|dev| (dev, plane_formats(&device)));
    crate::host::serve_dmabuf(state, &egl_display, &renderer, scanout);
    let import_node = state
        .render_node
        .as_deref()
        .and_then(|node| DrmNode::from_path(node).ok());
    state.renderer = Some(renderer);

    handle
        .insert_source(notifier, |event, _, state: &mut LiviState| match event {
            DrmEvent::VBlank(crtc) => vblank(state, crtc),
            DrmEvent::Error(e) => log::error!("display: {e}"),
        })
        .map_err(|e| e.error)?;

    let udev = UdevBackend::new(&seat)?;
    handle
        .insert_source(udev, |event, _, state: &mut LiviState| {
            if let UdevEvent::Changed { .. } = event {
                rescan(state);
            }
        })
        .map_err(|e| e.error)?;

    let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput
        .udev_assign_seat(&seat)
        .map_err(|()| format!("libinput cannot take seat {seat}"))?;
    handle
        .insert_source(
            LibinputInputBackend::new(libinput.clone()),
            |event, _, state: &mut LiviState| input(state, event),
        )
        .map_err(|e| e.error)?;

    state.seat.add_keyboard(Default::default(), 600, 25)?;
    state.seat.add_pointer();
    state.seat.add_touch();

    let connectors = connectors(&device, false);
    let gpu_wait = gpu_waiter(handle)?;

    log::info!("driving the displays directly on seat {seat}");
    Ok(DrmState {
        session,
        libinput,
        device,
        gbm,
        formats,
        import_node,
        loop_handle: handle.clone(),
        gpu_wait,
        frames: 0,
        connectors,
        outputs: Vec::new(),
        wanted: BTreeSet::from([0]),
        mode_settings: HashMap::new(),
        touch_outputs: HashMap::new(),
        active: true,
        mouse_active: false,
        cursor: cursor_image(),
        splash_until: Some(Instant::now() + SPLASH_WAIT),
        splash_timer: false,
    })
}

/// Waits for the GPU off the event loop: without a fence the display could take, a frame may only be flipped once the
/// GPU has drawn it (see `render`), and waiting in the loop would hold back input and clients for as long.
fn gpu_waiter(
    handle: &LoopHandle<'static, LiviState>,
) -> Result<GpuWait, Box<dyn std::error::Error>> {
    let (done, finished) = channel::channel::<(crtc::Handle, u64)>();
    handle
        .insert_source(finished, |event, _, state: &mut LiviState| {
            if let channel::Event::Msg((crtc, frame)) = event {
                drawn(state, crtc, frame);
            }
        })
        .map_err(|e| e.error)?;
    let (wait, frames) = std::sync::mpsc::channel::<(crtc::Handle, u64, SyncPoint)>();
    std::thread::Builder::new()
        .name("gpu-wait".into())
        .spawn(move || {
            for (crtc, frame, sync) in frames {
                if let Err(e) = sync.wait() {
                    log::warn!("waiting for the GPU: {e}");
                }
                if done.send((crtc, frame)).is_err() {
                    break;
                }
            }
        })?;
    Ok(wait)
}

fn open_device(
    session: &mut LibSeatSession,
    seat: &str,
) -> Result<(DrmDevice, DrmDeviceNotifier, DrmDeviceFd), Box<dyn std::error::Error>> {
    for path in udev::all_gpus(seat)? {
        let flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK;
        let fd = match session.open(&path, flags) {
            Ok(fd) => DrmDeviceFd::new(DeviceFd::from(fd)),
            Err(e) => {
                log::warn!("cannot open {}: {e}", path.display());
                continue;
            }
        };
        // Keep what the displays show (the boot splash) instead of switching them off: the first frame then
        // takes over with the same mode, without a modeset and without a black gap.
        let (device, notifier) = match DrmDevice::new(fd.clone(), false) {
            Ok(opened) => opened,
            Err(e) => {
                log::info!("{} drives no display: {e}", path.display());
                continue;
            }
        };
        if connectors(&device, false).iter().any(|c| c.connected) {
            log::info!("displays on {}", path.display());
            return Ok((device, notifier, fd));
        }
    }
    Err("no connected display".into())
}

fn plane_formats(device: &DrmDevice) -> FormatSet {
    let Ok(resources) = device.resource_handles() else {
        return FormatSet::default();
    };
    resources
        .crtcs()
        .iter()
        .filter_map(|crtc| device.planes(crtc).ok())
        .flat_map(|planes| planes.overlay.into_iter().chain(planes.primary))
        .flat_map(|plane| plane.formats.into_iter())
        .collect()
}

fn connectors(device: &DrmDevice, probe: bool) -> Vec<Connector> {
    let Ok(resources) = device.resource_handles() else {
        return Vec::new();
    };
    resources
        .connectors()
        .iter()
        .filter_map(|&handle| {
            let info = device.get_connector(handle, probe).ok()?;
            let crtcs = info
                .encoders()
                .iter()
                .filter_map(|&e| device.get_encoder(e).ok())
                .flat_map(|e| resources.filter_crtcs(e.possible_crtcs()))
                .collect();
            let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
            let edid = property(device, handle, "EDID")
                .and_then(|(_, blob)| device.get_property_blob(blob).ok());
            Some(Connector {
                handle,
                panel: edid.as_deref().and_then(panel_name).unwrap_or_default(),
                connected: info.state() == connector::State::Connected,
                modes: info.modes().to_vec(),
                panel_mm: info
                    .size()
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .map(|(w, h)| (w as i32, h as i32)),
                transform: orientation(device, handle),
                crtcs,
                name,
            })
        })
        .collect()
}

fn property(
    device: &DrmDevice,
    handle: connector::Handle,
    wanted: &str,
) -> Option<(smithay::reexports::drm::control::property::Info, u64)> {
    let props = device.get_properties(handle).ok()?;
    let (ids, values) = props.as_props_and_values();
    ids.iter().zip(values).find_map(|(&id, &value)| {
        let info = device.get_property(id).ok()?;
        (info.name().to_str() == Ok(wanted)).then_some((info, value))
    })
}

fn orientation(device: &DrmDevice, handle: connector::Handle) -> Transform {
    let Some((info, raw)) = property(device, handle, "panel orientation") else {
        return Transform::Normal;
    };
    let ValueType::Enum(values) = info.value_type() else {
        return Transform::Normal;
    };
    let Some(value) = values.get_value_from_raw_value(raw) else {
        return Transform::Normal;
    };
    match value.name().to_str() {
        Ok("Upside Down") => Transform::_180,
        Ok("Left Side Up") => Transform::_90,
        Ok("Right Side Up") => Transform::_270,
        _ => Transform::Normal,
    }
}

fn panel_name(edid: &[u8]) -> Option<String> {
    edid.get(54..126)?.chunks(18).find_map(|block| {
        if block[..3] != [0, 0, 0] || block[3] != 0xFC {
            return None;
        }
        let text: String = block[5..]
            .iter()
            .take_while(|&&b| b != 0x0A && b != 0)
            .map(|&b| b as char)
            .collect();
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    })
}

fn role_env(state: &LiviState, screen_idx: usize, what: &str) -> Option<String> {
    let role = state.screens.get(screen_idx)?.role.to_uppercase();
    let key = if what == "DISPLAY_MODE" && role == "MAIN" {
        "LIVI_DISPLAY_MODE".to_string()
    } else {
        format!("LIVI_{role}_{what}")
    };
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn mode_setting(state: &LiviState, screen_idx: usize) -> Option<String> {
    state
        .drm
        .as_ref()
        .and_then(|d| d.mode_settings.get(&screen_idx).cloned())
        .unwrap_or_else(|| role_env(state, screen_idx, "DISPLAY_MODE"))
}

pub fn set_mode(state: &mut LiviState, screen_idx: usize, setting: Option<String>) {
    let Some(drm) = state.drm.as_mut() else {
        return;
    };
    let picked = drm
        .outputs
        .iter()
        .position(|o| o.screen_idx == screen_idx)
        .and_then(|pos| {
            let output = &drm.outputs[pos];
            let connector = drm.connectors.iter().find(|c| c.name == output.connector)?;
            let mode = pick_mode(&connector.modes, setting.as_deref())?;
            (mode != output.mode).then_some(pos)
        });
    drm.mode_settings.insert(screen_idx, setting);
    if let Some(pos) = picked {
        drm.outputs.remove(pos);
        arrange(state);
    }
}

fn assign(wanted: &[(usize, Option<String>)], connected: &[String]) -> Vec<(usize, String)> {
    let is_free = |taken: &[(usize, String)], name: &str| !taken.iter().any(|(_, t)| t == name);
    let mut taken: Vec<(usize, String)> = Vec::new();
    let claimed: Vec<&String> = wanted
        .iter()
        .filter(|(s, _)| *s != 0)
        .filter_map(|(_, c)| c.as_ref())
        .collect();
    if let Some((_, configured)) = wanted.iter().find(|(s, _)| *s == 0) {
        let main = configured
            .as_ref()
            .filter(|n| connected.contains(n))
            .or_else(|| connected.iter().find(|n| !claimed.contains(n)))
            .or(connected.first());
        if let Some(name) = main {
            taken.push((0, name.clone()));
        }
    }
    for (screen, configured) in wanted.iter().filter(|(s, _)| *s != 0) {
        if let Some(name) = configured
            && connected.contains(name)
            && is_free(&taken, name)
        {
            taken.push((*screen, name.clone()));
        }
    }
    for (screen, _) in wanted.iter().filter(|(s, c)| *s != 0 && c.is_none()) {
        if let Some(name) = connected.iter().find(|n| is_free(&taken, n)) {
            taken.push((*screen, name.clone()));
        }
    }
    taken.sort();
    taken
}

pub fn open_screen(state: &mut LiviState, screen_idx: usize) {
    if let Some(drm) = state.drm.as_mut()
        && drm.wanted.insert(screen_idx)
    {
        arrange(state);
    }
}

pub fn close_screen(state: &mut LiviState, screen_idx: usize) {
    if screen_idx != 0
        && let Some(drm) = state.drm.as_mut()
        && drm.wanted.remove(&screen_idx)
    {
        arrange(state);
    }
}

fn rescan(state: &mut LiviState) {
    let Some(drm) = state.drm.as_mut() else {
        return;
    };
    drm.connectors = connectors(&drm.device, true);
    arrange(state);
}

fn arrange(state: &mut LiviState) {
    let Some(wanted) = state.drm.as_ref().map(|d| d.wanted.clone()) else {
        return;
    };
    let wanted: Vec<(usize, Option<String>)> = wanted
        .iter()
        .map(|&idx| (idx, role_env(state, idx, "CONNECTOR")))
        .collect();
    let Some(drm) = state.drm.as_mut() else {
        return;
    };
    let connected: Vec<String> = drm
        .connectors
        .iter()
        .filter(|c| c.connected)
        .map(|c| c.name.clone())
        .collect();
    let plan = assign(&wanted, &connected);
    let before = drm.outputs.len();
    drm.outputs
        .retain(|o| plan.contains(&(o.screen_idx, o.connector.clone())));
    let mut changed = drm.outputs.len() != before;
    for (screen_idx, name) in plan {
        let present = state
            .drm
            .as_ref()
            .is_some_and(|d| d.outputs.iter().any(|o| o.screen_idx == screen_idx));
        if present {
            continue;
        }
        let mode_setting = mode_setting(state, screen_idx);
        match light(state, screen_idx, &name, mode_setting.as_deref()) {
            Ok(()) => changed = true,
            Err(e) => log::error!("{name} for screen {screen_idx} did not light up: {e}"),
        }
    }
    if changed {
        crate::ctrl::send_displays(state);
        crate::ctrl::send_panels(state);
    }
    apply_calibration(state);
}

pub fn apply_calibration(state: &mut LiviState) {
    let Some(drm) = state.drm.as_ref() else {
        state.cal.in_lut = false;
        return;
    };
    let mut all = !drm.outputs.is_empty();
    for output in &drm.outputs {
        let size = drm
            .device
            .get_crtc(output.crtc)
            .map(|info| info.gamma_length() as usize)
            .unwrap_or(0);
        if size < 2 {
            log::info!(
                "{} has no gamma table, calibration runs in the shader",
                output.connector
            );
            all = false;
            continue;
        }
        let [r, g, b] = crate::cal::curves(&state.cal, size);
        if let Err(e) = drm.device.set_gamma(output.crtc, &r, &g, &b) {
            log::error!("{} refused the gamma table: {e}", output.connector);
            all = false;
        }
    }
    state.cal.in_lut = all;
}

fn light(
    state: &mut LiviState,
    screen_idx: usize,
    name: &str,
    mode_setting: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let drm = state.drm.as_mut().ok_or("no display device")?;
    let connector = drm
        .connectors
        .iter()
        .find(|c| c.name == name)
        .ok_or("connector gone")?;
    let mode = pick_mode(&connector.modes, mode_setting).ok_or("no mode")?;
    let busy: Vec<crtc::Handle> = drm.outputs.iter().map(|o| o.crtc).collect();
    let crtc = *connector
        .crtcs
        .iter()
        .find(|c| !busy.contains(c))
        .ok_or("no free crtc")?;
    let (handle, transform) = (connector.handle, connector.transform);
    let surface = drm.device.create_surface(crtc, mode, &[handle])?;
    let allocator = GbmAllocator::new(
        drm.gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let (w, h) = mode.size();
    let source = OutputModeSource::Static {
        size: Size::from((i32::from(w), i32::from(h))),
        scale: Scale::from(1.0),
        transform,
    };
    let surface = DrmCompositor::new(
        source,
        surface,
        None,
        allocator,
        GbmFramebufferExporter::new(drm.gbm.clone(), drm.import_node),
        [Fourcc::Argb8888, Fourcc::Xrgb8888],
        drm.formats.clone(),
        drm.device.cursor_size(),
        Some(drm.gbm.clone()),
    )
    .map_err(|e| e.to_string())?;
    let output = Output {
        screen_idx,
        connector: name.to_string(),
        crtc,
        mode,
        transform,
        surface,
        frame_pending: false,
        drawing: None,
        direct: 0,
        needs_redraw: true,
        held: Vec::new(),
    };
    let (lw, lh) = output.logical();
    drm.outputs.push(output);
    let screen = &mut state.screens[screen_idx];
    screen.width = lw;
    screen.height = lh;
    screen.fullscreen = true;
    screen.resize_pending = Some(Instant::now());
    crate::host::ensure_server_output(state, screen_idx);
    crate::layout::apply_ui_layout(state, screen_idx);
    log::info!(
        "screen '{}' on {name}: {w}x{h}@{} {transform:?}",
        state.screens[screen_idx].role,
        mode.vrefresh()
    );
    Ok(())
}

pub fn displays_lines(state: &LiviState) -> Option<String> {
    let drm = state.drm.as_ref()?;
    let shown: Vec<&Connector> = drm.connectors.iter().filter(|c| c.connected).collect();
    let mut out = format!("displays {}\n", shown.len());
    for c in shown {
        let output = drm.outputs.iter().find(|o| o.connector == c.name);
        let role = output
            .and_then(|o| state.screens.get(o.screen_idx))
            .map_or("-", |s| s.role.as_str());
        let current = output.map_or("-".to_string(), |o| {
            let (w, h) = o.mode.size();
            format!("{w}x{h}@{}", o.mode.vrefresh())
        });
        let modes = sizes(&c.modes);
        let modes = if modes.is_empty() {
            "-".to_string()
        } else {
            modes.join(",")
        };
        out.push_str(&format!(
            "display {} {role} {current} {modes} {}\n",
            c.name, c.panel
        ));
    }
    Some(out)
}

pub fn panel_mm(state: &LiviState, screen_idx: usize) -> Option<(i32, i32)> {
    let drm = state.drm.as_ref()?;
    let output = drm.outputs.iter().find(|o| o.screen_idx == screen_idx)?;
    let (w, h) = drm
        .connectors
        .iter()
        .find(|c| c.name == output.connector)?
        .panel_mm?;
    let turned = output
        .transform
        .transform_size(Size::<i32, smithay::utils::Physical>::from((w, h)));
    Some((turned.w, turned.h))
}

fn cursor_image() -> MemoryRenderBuffer {
    let mut pixmap = Pixmap::new(14, 22).expect("cursor pixmap");
    let mut path = PathBuilder::new();
    for (i, (x, y)) in [
        (1.0, 1.0),
        (1.0, 17.0),
        (5.0, 13.5),
        (8.0, 20.5),
        (10.5, 19.5),
        (7.5, 12.5),
        (12.5, 12.5),
    ]
    .into_iter()
    .enumerate()
    {
        if i == 0 {
            path.move_to(x, y);
        } else {
            path.line_to(x, y);
        }
    }
    path.close();
    let path = path.finish().expect("cursor path");
    let mut paint = Paint::default();
    paint.set_color(Color::WHITE);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );
    paint.set_color(Color::BLACK);
    let stroke = Stroke {
        width: 1.2,
        ..Default::default()
    };
    pixmap.stroke_path(
        &path,
        &paint,
        &stroke,
        tiny_skia::Transform::identity(),
        None,
    );
    crate::deco::to_buffer(&pixmap)
}

fn session_event(state: &mut LiviState, event: SessionEvent) {
    let Some(drm) = state.drm.as_mut() else {
        return;
    };
    match event {
        SessionEvent::PauseSession => {
            log::info!("session paused");
            drm.libinput.suspend();
            drm.device.pause();
            drm.active = false;
        }
        SessionEvent::ActivateSession => {
            log::info!("session active");
            if drm.libinput.resume().is_err() {
                log::error!("libinput did not resume");
            }
            if let Err(e) = drm.device.activate(false) {
                log::error!("display did not resume: {e}");
            }
            drm.active = true;
            for output in &mut drm.outputs {
                if let Err(e) = output.surface.reset_state() {
                    log::error!("{} did not reset: {e}", output.connector);
                }
                output.frame_pending = false;
                output.held.clear();
                output.drawing = None;
                output.needs_redraw = true;
            }
            rescan(state);
            pump(state);
        }
    }
}

fn vblank(state: &mut LiviState, crtc: crtc::Handle) {
    let Some(drm) = state.drm.as_mut() else {
        return;
    };
    let Some(output) = drm.outputs.iter_mut().find(|o| o.crtc == crtc) else {
        return;
    };
    if let Err(e) = output.surface.frame_submitted() {
        log::error!("frame not submitted: {e}");
    }
    output.frame_pending = false;
    output.held.clear();
    if output.needs_redraw {
        let idx = output.screen_idx;
        render(state, idx);
    }
}

/// The GPU has drawn the frame `render` left on `crtc`: it goes to the display now.
fn drawn(state: &mut LiviState, crtc: crtc::Handle, frame: u64) {
    let Some(drm) = state.drm.as_mut() else {
        return;
    };
    // Paused, or the output was reset or replaced in the meantime (VT switch, new mode): nothing waits for this frame
    // any more, and activating the session renders anew.
    if !drm.active {
        return;
    }
    let Some(output) = drm
        .outputs
        .iter_mut()
        .find(|o| o.crtc == crtc && o.drawing == Some(frame))
    else {
        return;
    };
    output.drawing = None;
    if let Err(e) = output.surface.queue_frame(()) {
        log::error!("display flip: {e}");
        return;
    }
    output.frame_pending = true;
    crate::host::send_frame_callbacks(state);
}

pub fn damage(state: &mut LiviState) {
    if let Some(drm) = state.drm.as_mut() {
        for output in &mut drm.outputs {
            output.needs_redraw = true;
        }
    }
}

pub fn pump(state: &mut LiviState) {
    let Some(drm) = state.drm.as_ref() else {
        return;
    };
    let due: Vec<usize> = drm
        .outputs
        .iter()
        .filter(|o| o.needs_redraw && !o.frame_pending && o.drawing.is_none())
        .map(|o| o.screen_idx)
        .collect();
    for idx in due {
        render(state, idx);
    }
}

/// Buffers currently attached to the surfaces (and subsurfaces) shown on a screen.
fn attached_buffers(state: &LiviState, screen_idx: usize) -> Vec<Buffer> {
    let mut buffers = Vec::new();
    for t in state
        .toplevels
        .iter()
        .filter(|t| t.screen_idx == screen_idx)
    {
        with_surface_tree_downward(
            t.toplevel.wl_surface(),
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_, states, _| {
                if let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>()
                    && let Some(buffer) = data.lock().unwrap().buffer()
                {
                    buffers.push(buffer.clone());
                }
            },
            |_, _, _| true,
        );
    }
    buffers
}

/// Marker file next to the control socket: the display shows LIVI's own frames from now on. A boot splash that keeps
/// its picture until then (Plymouth: "quit --retain-splash" removes its framebuffer, the screen would go black) can
/// end once it appears.
pub fn first_frame_marker(state: &LiviState) -> Option<std::path::PathBuf> {
    let ctrl = std::path::Path::new(state.ctrl_path.as_ref()?);
    Some(ctrl.with_file_name("compositor.first-frame"))
}

fn mark_first_frame(state: &LiviState) {
    if let Some(path) = first_frame_marker(state)
        && let Err(e) = std::fs::write(&path, b"")
    {
        log::warn!("{}: {e}", path.display());
    }
}

/// Whether LIVI's UI window on this screen has a buffer to show
fn ui_has_drawn(state: &LiviState, screen_idx: usize) -> bool {
    state
        .toplevels
        .iter()
        .filter(|t| t.kind == Kind::Ui && t.screen_idx == screen_idx)
        .any(|t| {
            with_states(t.toplevel.wl_surface(), |states| {
                states
                    .data_map
                    .get::<RendererSurfaceStateUserData>()
                    .is_some_and(|d| d.lock().unwrap().buffer().is_some())
            })
        })
}

/// Flicker-free start: no frame until LIVI's UI has drawn, so the boot splash stays on the display instead of the
/// backdrop colour (the UI needs a few seconds for its first frame; the video planes come earlier, with black
/// buffers, and must not end the wait). After SPLASH_WAIT the backdrop is shown in any case.
fn keep_splash(state: &mut LiviState, screen_idx: usize) -> bool {
    let client = ui_has_drawn(state, screen_idx);
    let Some(drm) = state.drm.as_mut() else {
        return false;
    };
    let Some(until) = drm.splash_until else {
        return false;
    };
    if client || Instant::now() >= until {
        drm.splash_until = None;
        if client {
            log::info!("first frame of the UI");
        } else {
            log::info!("first frame without the UI (splash wait over)");
        }
        mark_first_frame(state);
        return false;
    }
    if !drm.splash_timer {
        drm.splash_timer = true;
        let _ = drm.loop_handle.insert_source(
            Timer::from_deadline(until),
            |_, _, state: &mut LiviState| {
                damage(state);
                pump(state);
                TimeoutAction::Drop
            },
        );
    }
    true
}

pub fn render(state: &mut LiviState, screen_idx: usize) {
    let ready = state.drm.as_ref().is_some_and(|d| {
        d.active
            && d.outputs.iter().any(|o| {
                o.screen_idx == screen_idx
                    && !o.frame_pending
                    && o.drawing.is_none()
                    && o.needs_redraw
            })
    });
    if !ready || state.renderer.is_none() {
        return;
    }
    let mut parts = crate::render::frame_parts(state, screen_idx);
    if keep_splash(state, screen_idx) {
        return;
    }
    let held = attached_buffers(state, screen_idx);
    let screen_x = state.screens[screen_idx].x as f64;
    let (Some(drm), Some(renderer)) = (state.drm.as_mut(), state.renderer.as_mut()) else {
        return;
    };
    let cursor_shown =
        screen_idx == 0 && drm.mouse_active && crate::host::shown_cursor(&state.host).is_some();
    if cursor_shown {
        let (x, y) = state.host.pointer_pos;
        let at = Point::<f64, smithay::utils::Physical>::from((x - screen_x, y));
        if let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at,
            &drm.cursor,
            None,
            None,
            None,
            ElementKind::Cursor,
        ) {
            parts.elements.insert(0, LiviElement::Deco(element));
        }
    }
    let Some(output) = drm.outputs.iter_mut().find(|o| o.screen_idx == screen_idx) else {
        return;
    };
    output.needs_redraw = false;
    let clear = parts.clear;
    let elements = match crate::render::calibrated(renderer, parts, output.logical()) {
        Ok(elements) => elements,
        Err(e) => {
            log::error!("calibration failed: {e}");
            return;
        }
    };
    let flipped = match output
        .surface
        .render_frame(renderer, &elements, clear, FrameFlags::DEFAULT)
    {
        Ok(frame) if frame.is_empty => false,
        Ok(frame) => {
            let direct = frame.overlay_elements.len()
                + usize::from(matches!(
                    frame.primary_element,
                    PrimaryPlaneElement::Element(_)
                ));
            if direct != output.direct {
                log::info!(
                    "screen '{}' on {}: {direct} of {} surfaces shown directly by the display",
                    state.screens[screen_idx].role,
                    output.connector,
                    elements.len()
                );
                output.direct = direct;
            }
            // Without a fence the display takes for the GPU's work (no syncobj support in the display driver, or no
            // fence fd from EGL), the flip must wait until the GPU has drawn the frame; otherwise the display may show
            // the buffer half drawn, with parts of what it held frames ago. Only implicit sync in the kernel would
            // cover it, and not every GPU driver puts fences on its buffers (PowerVR's DDK does not).
            if frame.needs_sync()
                && let PrimaryPlaneElement::Swapchain(element) = &frame.primary_element
                && !element.sync.is_reached()
            {
                drm.frames += 1;
                if drm
                    .gpu_wait
                    .send((output.crtc, drm.frames, element.sync.clone()))
                    .is_ok()
                {
                    output.drawing = Some(drm.frames);
                    // the GPU reads them now; released on the vblank after the flip in `drawn`
                    output.held = held;
                    return;
                }
                if let Err(e) = element.sync.wait() {
                    log::warn!("waiting for the GPU: {e}");
                }
            }
            match output.surface.queue_frame(()) {
                Ok(()) => true,
                Err(e) => {
                    log::error!("display flip: {e}");
                    return;
                }
            }
        }
        Err(e) => {
            log::error!("render failed: {e}");
            return;
        }
    };
    if flipped {
        output.frame_pending = true;
        output.held = held;
        crate::host::send_frame_callbacks(state);
        return;
    }
    let refresh = output.mode.vrefresh().max(1);
    let _ = drm.loop_handle.insert_source(
        Timer::from_duration(std::time::Duration::from_millis(u64::from(1000 / refresh))),
        |_, _, state: &mut LiviState| {
            crate::host::send_frame_callbacks(state);
            TimeoutAction::Drop
        },
    );
}

#[cfg(test)]
mod tests;
