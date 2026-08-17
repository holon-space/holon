---
id: 2026-07-18-android-gpui-toolbar-menu-icons-render
date: 2026-07-18
gap: ENVIRONMENT
secondary: PERCEPTION
status: FIXED
summary: >-
  Android GPUI toolbar/menu icons render as tofu boxes (□) — the icon font is
  not bundled/loaded in the APK, so glyph codepoints fall back to the
  missing-glyph box (screenshot evidence, on-device dogfooding)
source_line: 804
---

## Bug

Android GPUI toolbar/menu icons render as tofu boxes (□) — the icon font is
not bundled/loaded in the APK, so glyph codepoints fall back to the
missing-glyph box (screenshot evidence, on-device dogfooding)

## Missing piece

keystone never runs the Android platform stack nor exercises APK asset
bundling / font loading, so an unpackaged icon font is invisible to it;
needs an on-device (or APK-asset) parity check that required icon fonts are
packaged and resolve

## Remedy

FIXED 2026-07-19 (holon-side; gpui-mobile/zed pins untouched). ROOT CAUSE
(two classes): (1) the toolbar's monochrome symbols (☰ ◧ ⚙, chevrons ▸▾,
checkboxes ☑☐, op-buttons ✎✕, dismiss ✕) live in the Misc-Symbols /
Geometric-Shapes / Dingbats blocks, which Android's bundled Roboto/Noto Sans
(the only fonts gpui-mobile's `AndroidPlatform::new` loads from
`/system/fonts`) do NOT cover → tofu. (2) the two toolbar EMOJI (🎨 gallery,
🔗 accept-ticket) plus 🔍/🔎/⛔ need color-emoji, but the device NotoColorEmoji
is COLR v1 which gpui-mobile's swash cannot rasterise AND the `just apk`
build packs NO assets at all (`aapt2 link` has no `-A`, the zip only adds
the two `.so`s), so the fork's
`load_asset_bytes("fonts/NotoColorEmoji.ttf")` CBDT path can never resolve.
So APK-asset bundling was a dead end; the font must ride inside the `.so`.
FIX: embed DejaVu Sans (permissive Bitstream Vera + public-domain license,
shipped UNMODIFIED at `assets/fonts/DejaVuSans.ttf` + `LICENSE_DEJAVU`, ~756
KB) via `include_bytes!` and register it Android-only with
`cx.text_system().add_fonts(...)` in `mobile::register_android_icon_fonts`
(called at the top of `open_holon_window` before first paint); cosmic-text's
per-glyph resolution then covers the DejaVu-covered monochrome symbols
app-wide (verified from the font cmap: ☰◧⚙ ▸▾▶▼ ✕✎✓☑☐ ⚠↻ ⟳⇥⇤↑↓ ‹›⌃⌄ ◉●○ − ℹ
⟨⟩ ▦ ◌ ⌂ ⠿ • and more). Glyphs DejaVu genuinely LACKS — the emoji 🎨🔗🔍🔎⛔🗑 AND
the monochrome-but-uncovered ⧉ — are mapped by `crate::icon()` (lib.rs
`ICON_SUBSTITUTES`) to a DejaVu-covered substitute ON ANDROID ONLY: 🎨→▦,
🔗→⚭, 🔍/🔎→⚲, ⛔→⊘, 🗑→⌦ (delete op), ⧉→❐ (embed op); mac/iOS keep the original
glyph byte-identical. All name→glyph tables now route through `icon()`:
`op_button::OP_ICONS` and the semantic `icon::ICON_CHARS`. Residual
documented gap: `icon::ICON_CHARS` also holds 🔒/🔓 (lock/unlock) for which
DejaVu has NO padlock glyph and no acceptable substitute — recorded in
`crate::KNOWN_ANDROID_GLYPH_GAPS`; these are currently unreachable (no
layout names `lock`/`unlock`; the widget gallery uses lucide SVG names that
fall through to `•`), and if ever wired need an SVG icon or a lock-capable
font, not a misleading swap. Fails loud: an `add_fonts` error is
`log::error!`d, never swallowed. Guarded host-side by four coverage tests
(`cargo test -p holon-gpui --lib`):
`icon_font_tests::{inline_ui_glyphs_render_on_android,
substitutes_are_covered_and_needed}` (lib.rs sweeps `INLINE_UI_GLYPHS` + the
substitute-table invariants),
`op_button::op_icon_coverage::every_op_glyph_renders_on_android`, and
`icon::icon_char_coverage::every_named_icon_glyph_renders_on_android` — the
last two sweep the glyph TABLES programmatically (closing the earlier hole
where a hand-maintained glyph list missed op_button, letting 🗑/⧉ escape).
Each asserts every glyph is DejaVu-covered after Android substitution or a
documented gap. Gates: `cargo check -p holon-gpui` (desktop) clean; `cargo
test -p holon-gpui --lib` icon tests 4/4 green; `cargo ndk` check of
`--no-default-features --features mobile` for aarch64-linux-android green
(no new warnings). GAP REMEDY still open: no on-device/emulator parity gate
asserting required icon glyphs actually rasterise (non-tofu) at a real
viewport — full on-device visual confirmation is deferred to the
orchestrator.
