# Vendored crates

## femtovg 0.27.0

- Text up to 384 px goes through the glyph atlas, upstream switches to path filling above 92 px.
- Path filling leaves a step on flat top edges of large glyphs (P, 7 in the dashboard).
- Drop the copy once upstream renders large text cleanly.
- Its Cargo.toml allows `unused_imports` and `dead_code`: with the default features off, as Slint builds it, upstream leaves `FilePath` and `font_scale` unused. A path dependency shows those warnings, the registry copy would hide them.

## i-slint-renderer-skia 1.18.1

- `opengl_surface.rs`: `LIVI_UI_FINISH=1` waits with `glFinish` until the GPU has drawn a frame, after Skia's flush and before
  the swap. Off by default; nothing else changed against crates.io.
- Why: some drivers pass the buffer to the compositor without a fence for drawing still under way (PowerVR DDK 1.11 on the
  GE8300, Wayland). The compositor then shows it half drawn or the one before, and the picture flickers between two
  buffers. Measured on that board: with the wait the buffers no longer alternate, at the same frame rate.
- Not by a rendering notifier: one on the main window turns Slint's partial rendering off (see `livi-ui/src/main.rs`).
- Drop the copy once Slint offers a wait before the swap, or the driver keeps its fence.

