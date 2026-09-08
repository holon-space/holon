---
id: 2026-09-08-long-conflict-copy-paints-no-badge-at-all
date: 2026-09-08
gap: COVERAGE
secondary: PERCEPTION
status: OPEN
summary: >-
  A pairing conflict copy whose content fills the row width paints no badge at
  all — the "kept from this device before pairing" mark disappears exactly for
  the long blocks, so the merge it exists to disclose is silent again.
---

## Bug

Found by the `dogfood-explorer` gate for D93.a (pair-conflict-badge), port 8710,
throwaway vault `/tmp/dogfood-w10-badge` seeded with
`lane-logs/dogfood-w10/seed/pair-conflict.org`.

The seed holds two blocks carrying `pairing_conflict_of`: a short one ("the
words this device wrote") and one whose content is a 155-character line. Both
resolve to the `pairing_conflict` profile variant, and `describe_ui` shows a
`badge "kept from this device before pairing"` node under BOTH rows.

Only the short one is on screen.

- Short copy: `rendered_text … w=599.0` inside an 824 px row — 225 px are left
  for the badge, and the badge paints (screenshot
  `lane-logs/dogfood-w10/01-crop-rows-s.png`).
- Long copy: `rendered_text … w=824.0 h=56.0` — the text takes the full row
  width, exactly like a block with no badge, and nothing is painted to its
  right (screenshot `lane-logs/dogfood-w10/02-crop-long-s.png`).

Full geometry dump: `lane-logs/dogfood-w10/geom-main.txt` — it carries
`tree_item`, `column`, `selectable`, `draggable`, `state_toggle`,
`rendered_text` and `drop_zone` rows for `block:pair-conflict-long-before-pairing`
and no badge row.

Severity is the feature's whole point: the property is the ONLY mark a conflict
copy carries (a marked title would be written back into the org file), so a copy
whose text happens to be long merges silently — the condition D93.a exists to
prevent.

## Root cause

The variant renders the badge as the last child of the same `row(...)` that
holds `rendered_text(col("content"))`
(`assets/default/types/block_profile.yaml:157`). `rendered_text` grows to
consume the row, so the badge's share of the width shrinks to nothing once the
content is long enough — there is no `flex_shrink(0)` on the badge, no minimum
width, and no wrap onto a second line. The badge element is still built
(`frontends/gpui/src/render/builders/badge.rs`), which is why the render tree
shows it and the screen does not.

Window width was the default (1512 logical px); a narrower window makes the
threshold lower, not the behaviour different.

## Missing piece

`frontends/gpui/tests/pairing_conflict_badge_windowed.rs` seeds only short
content (`KEPT_TEXT = "the words this device wrote"`, `PAGE_COPY_TEXT = "a page
shaped copy"`), so the badge always has room. The assertion mechanism is
already right — it reads painted badges out of `BoundsRegistry` — and would
have gone red for the right reason on a long-content case. Nothing generates
one: the windowed test has fixed seeds and the headless tier has no pixels.

## Remedy

Open. Two parts:

1. Give the badge a non-shrinking slot (or let the row wrap) so the mark
   survives any content length.
2. Add a long-content conflict copy to
   `pairing_conflict_badge_windowed.rs` — the same assertion, a content string
   wide enough to fill the row — as the red-for-the-right-reason proof before
   the fix.
