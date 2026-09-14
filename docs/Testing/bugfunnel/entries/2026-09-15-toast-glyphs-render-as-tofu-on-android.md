---
id: 2026-09-15-toast-glyphs-render-as-tofu-on-android
date: 2026-09-15
gap: COVERAGE
status: FIXED
summary: >-
  Three shipping disclosures drew their icon with a glyph the embedded Android
  font cannot render, so on Android the key, ticket and blocked marks painted as
  tofu boxes — the glyph-coverage sweep only swept a hand-maintained list that
  the toast table was never added to.
---

## Bug

On Android, three conditions painted their toast with an unrenderable glyph:

| Condition | Glyph | Codepoint |
|---|---|---|
| `SecretsHeldInMemory` | 🔑 | U+1F511 |
| `OwnerRecoveryCodeNotShown` | 🔑 | U+1F511 |
| `BearerTicketEnrollment` | 🎟 | U+1F39F |

The embedded `assets/fonts/DejaVuSans.ttf` covers neither codepoint, and
neither had a row in `ICON_SUBSTITUTES`, so `icon()` passed them through
unchanged and the renderer drew a replacement box. The headline and colour were
correct; only the mark was missing, which is why nobody reported it.

Found on 2026-09-15 by a NEW coverage test written while moving the toast glyph
table into `ConditionProfile` (lane `error-remedy`, ruling R1). The three
glyphs are on shipped code paths, so this had escaped to every Android build
since those conditions were added.

## Root cause

The icon-font coverage tests sweep three sources: the two name→glyph tables
(`op_button::OP_ICONS`, `icon::ICON_CHARS`) and one hand-maintained list of
inline literals, `INLINE_UI_GLYPHS` (`frontends/gpui/src/lib.rs`).

The GPUI toast table (`share_ui::toast_style`) was a fourth source and was in
none of them. Three of its glyphs — ⚠, ↻, ⛔ — happened to be listed in
`INLINE_UI_GLYPHS` by hand, which made the table look covered. The other four
— 🔑, ↩, 🎟, `i` — were never added, and nothing could notice: the list's own
doc comment calls out that "missing an entry is the one drift risk", and this is
that risk realised.

⛔ is also uncovered by DejaVu but already had a substitute row, which is why it
painted correctly and the two that did not were indistinguishable from it in
review.

## Missing piece

**COVERAGE.** No test enumerated the glyphs the toast table could draw. The
oracle was correct and available — `assert_icon_renderable_on_android` is the
exact check, and it was already applied to three other sources — but nothing
fed the toast table's glyphs into it. A hand-maintained list of literals cannot
be a coverage boundary, because adding a glyph and adding it to the list are two
separate acts.

## Remedy

Fixed in the same change that surfaced it.

The glyph vocabulary is now declared as named constants in
`crates/holon-api/src/condition_profile.rs` (`icons::*`), with every distinct
value enumerated in `CONDITION_ICONS`. A new test,
`icon_font_tests::condition_icons_render_on_android`, sweeps that constant, so a
profile added in `holon-api` cannot reach a device with a glyph nothing can
render. The three now-migrated entries were removed from `INLINE_UI_GLYPHS`,
which no longer double-counts them.

Two substitution rows close the gap itself: 🔑 → ⚷ (U+26B7, the key-shaped
Chiron sign) and 🎟 → ▭ (U+25AD, an empty rectangle reading as a ticket stub).
Both substitutes are covered by DejaVu and both sources are not, which is what
`substitutes_are_covered_and_needed` requires.

Red log: `lane-logs/icon-coverage-1789425149.log` —
`CONDITION_ICONS: icon "🔑" → Android-effective "🔑" has char U+1F511 that
DejaVu Sans cannot render and no substitute covers`.
Green log: `lane-logs/icon-coverage-1789425296.log` — `3 tests run: 3 passed`.
