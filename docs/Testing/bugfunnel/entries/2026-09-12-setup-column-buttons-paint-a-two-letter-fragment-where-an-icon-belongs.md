---
id: 2026-09-12-setup-column-buttons-paint-a-two-letter-fragment-where-an-icon-belongs
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Every Settings › Integrations action button paints a large "OP" or "CO" above
  its own label, because the op-icon table has no glyph for `open` or
  `configure` and the fallback is the first two letters of the label.
---

## Bug

Found by the `user-connections` dogfood RE-CHECK of 2026-09-12, driving the
real GPUI app at `main` `060022da56b6`.

In Settings › Integrations the Setup column carries one button per available
operation. The first, "Switch integration", paints a pencil glyph above its
label, which is the intended shape. The other two paint a letter pair in the
glyph's place, in the glyph's size and weight:

```
   ✎              CO                 OP
Switch        Configure…            Open
integration
```

Read at a glance it looks like a rendering failure — a large black `CO` sitting
on top of a small grey `Configure…`, the same two letters twice. Every bundled
provider row shows it; `gcal` and `gmail` show both variants at once.

Evidence under `scratchpad/dogfood-uc-recheck/shots/`:

- `A-03-setupzoom.png` — the `gcal` and `gmail` rows magnified.
- `A-03-table.png`, `B-01-table-800.png` — the whole table at 1400 and 800.

## Root cause

`frontends/gpui/src/render/builders/op_button.rs:414` looks the op name up in
the `OP_ICONS` table and returns an empty string for an unknown one; the caller
then falls back to `fallback_short_label`
(`frontends/gpui/src/render/builders/op_button.rs:422`), the first two
alphanumeric characters of the display name, uppercased.

`OP_ICONS` covers the outline operations (delete, embed, indent, move…). The
integration operations `open` and `configure` are not in it, so both take the
fallback. The fallback is deliberate and documented in the doc comment above
`op_icon_char` — it exists so an op is never a blank button — but it was
designed for a button whose label is elsewhere, not for one that paints the
label directly underneath.

## Missing piece

`op_icon_coverage::every_op_glyph_renders_on_android` sweeps the glyphs that
ARE in the table and asserts each renders. Nothing asserts the other
direction — that every op a surface actually offers HAS a glyph — so adding a
new op silently opts it into the two-letter fallback with no test going red.

## Remedy

Open. Two pieces:

1. Give `open` and `configure` glyphs in `OP_ICONS`.
2. Turn the coverage test around: enumerate the ops the shipped surfaces offer
   and assert each resolves to a glyph, so the fallback stays the emergency it
   was meant to be rather than the normal case for a whole column.

## Fix

The glyph an op button paints is now tracked in its own right.
`frontends/gpui/src/render/builders/op_button.rs` registers it as
`op-icon-{op}-{target}` with its displayed text, so a window can tell a real
glyph from the emergency two-letter short label. Before this, the button's only
tracked text was its LABEL, which is the same string either way — so a whole
column rendering `CO` over `Configure…` passed every rung there was.

`begin_oauth` was the op with no entry in `OP_ICONS`; it has a glyph now.

Covered by `frontends/gpui/tests/settings_integrations_ops_windowed.rs`.

- RED `lane-logs/red-settings_integrations_ops_windowed-1789379944.log`.
- GREEN `lane-logs/green-settings_integrations_ops_windowed-1789389949.log`.
- TEETH `lane-logs/teeth-icon-settings_integrations_ops_windowed-1789390571.log`
  — the tracked glyph text blanked; red naming `begin_oauth` and listing every
  painted glyph; restored byte-for-byte.
