---
id: 2026-10-01-a-heading-moved-into-journals-is-expected-after-the-day-page
date: 2026-10-01
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The keystone model put a heading that an external editor appended to
  journals.org after the day page, which has its own file; the SUT correctly
  puts it after the file's last heading, before the day page.
---

## Bug
Keystone runs of the D229 lane (rounds 2 to 12) failed with
`inv-live-children-match-ref` / `inv-loro-children-match-ref`: `sibling order
diverges under parent block:journals`, ref `[auto-create, <day page>, X]`, SUT
`[auto-create, X, <day page>]`, X being a heading cut from another page file
(`c1`, `c2`, `fe-parent`, `fe-target`, `bulk-1-1`). Run 1 of round 12c shrank it
to one transition, `MoveBlockBetweenFiles{c1, structural-page -> journals,
SourceFirst}`, deterministic on round 11 and 12 (`lane-logs/d229r12d-movej-r11.log`,
`lane-logs/d229r12d-movej-lane.log`). The known-reds classifier filed every
occurrence under `bulk-add-sibling-order`, whose pattern matched any sibling-order
divergence, so the red was never triaged.

## Root cause
The model was wrong, not the SUT. `MoveBlockBetweenFiles::apply_to_ref` placed the
moved subtree with `Placement::Last`, after every child of the target page. The
editor appends the heading to the end of the target's FILE, and the ingest places
a file heading after its predecessor in that file, skipping children de-inlined
into their own files (`BlockDelta::Upsert::prev` and the `after_block_id`
predecessor walk in `crates/holon-filesystem/src/file_sync_controller.rs`; Model.md
invariant 3: intent carries `after_sibling`). The day page lives in
`Journals/<date>.org`, not in journals.org, so the file cannot place the heading
after it: the heading lands directly after `auto-create`.

## Missing piece
A model placement for "appended to the target's file" (it equals "last child"
only for pages with no child page that has its own file), and a classifier
pattern bound to its own shape.

## Remedy
`Placement::LastInFile` (`crates/holon-integration-tests/src/pbt/block_state.rs`):
after the last child the target's own file holds. Pinned by three hand-authored
cases `a-heading-moved-into-journals-lands-before-the-day-page-*` (loro source
first and target first, sqlonly source first): green
`lane-logs/d229r12e-cases-green.log`; teeth (file filter disabled in place) red
with the original divergence `lane-logs/d229r12e-cases-teeth.log`, restore
sha256-verified. `bulk-add-sibling-order` now matches only a SUT order that
starts with a `block:bulk-` block; replay of every lane log
(`lane-logs/d229r12e-classify-before.log` vs `-after.log`): the documented
bulk-add signature still classifies, the journals shape is novel.
