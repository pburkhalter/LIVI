# Vendored crates

## femtovg 0.27.0

- Text up to 384 px goes through the glyph atlas, upstream switches to path filling above 92 px.
- Path filling leaves a step on flat top edges of large glyphs (P, 7 in the dashboard).
- Drop the copy once upstream renders large text cleanly.
- Its Cargo.toml allows `unused_imports` and `dead_code`: with the default features off, as Slint builds it, upstream leaves `FilePath` and `font_scale` unused. A path dependency shows those warnings, the registry copy would hide them.

## i-slint-renderer-skia 1.18.1

- `opengl_surface.rs`: `LIVI_UI_FINISH=1` waits with `glFinish` until the GPU has drawn a frame, after Skia's flush and before
  the swap. Off by default.
- `lib.rs`, `partial_rendering_state`: partial rendering (`SLINT_SKIA_PARTIAL_RENDERING`) stays on for a window with a
  rendering notifier. Upstream turns it off for any notifier, as one might draw into the frame; LIVI's draw nothing.
  The main window gets one only with the lever "UI hole" (`livi-ui/src/ui_hole.rs`), which would otherwise redraw the
  whole window each frame; the secondary windows have one to attach their video. Nothing else changed against
  crates.io.
- Why: some drivers pass the buffer to the compositor without a fence for drawing still under way (PowerVR DDK 1.11 on the
  GE8300, Wayland). The compositor then shows it half drawn or the one before, and the picture flickers between two
  buffers. Measured on that board: with the wait the buffers no longer alternate, at the same frame rate.
- Not by a rendering notifier: the wait is wanted on every frame, also without the lever "UI hole".
- Drop the copy once Slint offers a wait before the swap, or the driver keeps its fence.

## smithay 0.7.0

- `OutputDamageTracker::set_full_damage` and `DrmCompositor::set_full_damage`: a frame that draws its whole target anew may first tell the GPU that the old contents are not needed (`FullDamage`, `Frame::begin_full_damage`). Smithay neither clears nor invalidates, so a tile-based GPU loads the target from memory although everything is drawn over. The default `FullDamage::Keep` changes nothing.
- Everything else is the crate as published; the difference is `native/vendor/smithay/` against `smithay-0.7.0` on crates.io.
- Drop the copy once upstream has such a call, or when the lever proves to bring nothing (`LIVI_COMPOSITOR_INVALIDATE`, livi-compositor).
