# Vendored crates

## femtovg 0.27.0

- Text up to 384 px goes through the glyph atlas, upstream switches to path filling above 92 px.
- Path filling leaves a step on flat top edges of large glyphs (P, 7 in the dashboard).
- Drop the copy once upstream renders large text cleanly.
- Its Cargo.toml allows `unused_imports` and `dead_code`: with the default features off, as Slint builds it, upstream leaves `FilePath` and `font_scale` unused. A path dependency shows those warnings, the registry copy would hide them.

## smithay 0.7.0

- `OutputDamageTracker::set_full_damage` and `DrmCompositor::set_full_damage`: a frame that draws its whole target anew may first tell the GPU that the old contents are not needed (`FullDamage`, `Frame::begin_full_damage`). Smithay neither clears nor invalidates, so a tile-based GPU loads the target from memory although everything is drawn over. The default `FullDamage::Keep` changes nothing.
- Everything else is the crate as published; the difference is `native/vendor/smithay/` against `smithay-0.7.0` on crates.io.
- Drop the copy once upstream has such a call, or when the lever proves to bring nothing (`LIVI_COMPOSITOR_INVALIDATE`, livi-compositor).
