# Vendored crates

## femtovg 0.27.0

- Text up to 384 px goes through the glyph atlas, upstream switches to path filling above 92 px.
- Path filling leaves a step on flat top edges of large glyphs (P, 7 in the dashboard).
- Drop the copy once upstream renders large text cleanly.
- Its Cargo.toml allows `unused_imports` and `dead_code`: with the default features off, as Slint builds it, upstream leaves `FilePath` and `font_scale` unused. A path dependency shows those warnings, the registry copy would hide them.

## i-slint-renderer-skia 1.18.1

- `lib.rs`, `partial_rendering_state`: partial rendering (`SLINT_SKIA_PARTIAL_RENDERING`) stays on for a window with a
  rendering notifier. Upstream turns it off for any notifier, as one might draw into the frame; LIVI's draw nothing.
  Every window of LIVI's has one: the main window after each frame for the frame count, `LIVI_UI_FINISH`
  (`livi-ui/src/gpu_finish.rs`) and the lever "UI hole" (`livi-ui/src/ui_hole.rs`); the secondary windows to attach
  their video and for `LIVI_UI_FINISH`. Without the copy each of them would redraw the whole window every frame. Nothing
  else changed against crates.io.
- Drop the copy once Slint keeps partial rendering with a notifier that draws nothing (an opt-in, to be proposed).
