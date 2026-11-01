# Vendored crates

## femtovg 0.27.0

- Text up to 384 px goes through the glyph atlas, upstream switches to path filling above 92 px.
- Path filling leaves a step on flat top edges of large glyphs (P, 7 in the dashboard).
- Drop the copy once upstream renders large text cleanly.
- Its Cargo.toml allows `unused_imports` and `dead_code`: with the default features off, as Slint builds it, upstream leaves `FilePath` and `font_scale` unused. A path dependency shows those warnings, the registry copy would hide them.

## i-slint-renderer-skia 1.18.1 (branch probe/buffer-age only, never merged)

- A copy for the measurements P126 and P131 of the T100, see `docs/messungen/2026-11-01_flacker-pruefbau.md` in its repo.
- `opengl_surface.rs` and `lib.rs` carry switches, without them it draws and swaps as upstream does.
