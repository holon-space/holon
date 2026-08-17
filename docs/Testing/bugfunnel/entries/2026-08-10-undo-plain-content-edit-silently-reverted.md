---
id: 2026-08-10-undo-plain-content-edit-silently-reverted
date: 2026-08-10
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Undo of a plain content edit is silently reverted when focus leaves the
  block: the open editor's un-re-seeded buffer commits over the undone store
  on blur.
source_line: 744
---

## Bug

(task #80 lane, found by DOGFOODING — the task #68 dogfood-explorer gate,
finding F1, reproduced twice by hand; no automated test produced it) **Undo
of a plain content edit is silently reverted when focus leaves the block:
the open editor's un-re-seeded buffer commits over the undone store on
blur.** Driven live at main `1bda6435` in the SHIPPED SqlOnly default
(`app.log` line 1: `loro: false`): click `block:defvocab-b2` ("alpha two"),
type `QQ`, Cmd-Z, then click a different row. The app's own log carries the
whole chain verbatim — `set_field value="alpha twoQ" write_seq=37` and
`..."alpha twoQQ" write_seq=38` (the two keystrokes, editor-stamped), then
at 14:12:49.145 `set_field value="alpha two"` with NO `write_seq` (the
undo's inverse — `SqlOperationProvider::set_field_inverse` omits the token),
then at 14:12:52.245 `set_field origin=user value="alpha twoQQ"` with NO
`write_seq` — a `ViewEventHandler::handle_text_sync` BLUR commit, not a
keystroke, 3.1s later on the click that moved focus away. Store and render
agreed right up to the blur, so nothing warned and nothing logged. WHY THE
INSTALLED FIX DID NOT COVER IT: `ReseedGesture` (landed in this same
`1bda6435`) exists precisely for this seam — a focused SqlOnly editor is
skipped by the render backstop (`converge_on_render`,
`frontends/gpui/src/render/builders/editable_text.rs:249`) and its per-row
data subscription is orphaned by any row-set rebuild — and it is armed
GENERICALLY by every undo (`frontends/gpui/src/share_ui.rs:896,915`), not
only by promotion undos. The rung this lane adds proves the mechanism works
under test conditions, so the live session met a condition the harness does
not reproduce; the dogfood's own log flags `[interaction-pump]
WINDOW-INACTIVE` on the very click that produced the clobbering commit
(finding F6), which is the leading suspect since both the re-seed refusal
(`FocusMoved`) and the backstop key on focus predicates. NOT a #68
regression and NOT closed by #68: the blur-commit-over-external-write seam
is untouched by the promotion work, and the dogfood binary DID carry
`1bda6435` (its step 3a records `TODO alpha one` at len 14, the D1 fix's own
signature — pre-fix it fused to `TODOalpha one`; app start 14:02:38 UTC vs
the rev committed 13:56:26 UTC).

## Root cause

task #80 lane, found by DOGFOODING — the task #68 dogfood gate's finding F1,
reproduced twice by hand against a live app at main `1bda6435` in the
shipped SqlOnly default: **undo of a plain content edit is silently reverted
when focus leaves the block**, the open editor's un-re-seeded buffer
committing over the undone store on blur. The app log carries the chain
verbatim — two editor keystrokes stamped `write_seq` 37/38, the undo's
inverse `set_field "alpha two"` with no `write_seq`, then 3.1s later a
`set_field origin=user "alpha twoQQ"` with no `write_seq` on the click that
moved focus away: a blur commit, not a keystroke. Primary COVERAGE because
no rung at any layer followed an undo with a FOCUS CHANGE and then asserted
the store, and the headless keystone structurally cannot — its mirror models
the data-sync loop as always-delivering and drives no blur commit at all.
Secondary ENVIRONMENT because the windowed rung this lane adds
(`frontends/gpui/tests/undo_survives_blur_windowed.rs`, three arms,
mutation-proven) is GREEN at main, so the live condition that defeats the
generically-armed `ReseedGesture` is not reproducible in the harness — the
dogfood's own `WINDOW-INACTIVE` flag on the clobbering click is the leading
suspect. NOT a #68 regression and NOT closed by #68; row status OPEN, pinned
not fixed.)

## Missing piece

COVERAGE (primary): no automated test at any rung followed an `undo` with a
FOCUS CHANGE and then asserted the store. The windowed rungs pin
undo-then-KEYSTROKE (`live_promotion_windowed.rs`, the reseed set); the
headless keystone cannot express either half —
`HeadlessEditorMirror::converge_editor` is called unconditionally from the
harness settle, so it models the data-sync loop as ALWAYS delivering and a
focused editor there can never hold a stale buffer, and the mirror drives no
blur commit at all (`vm_commit_edit` commits per keystroke; `on_blur` is
never called headlessly). Missing piece, part 1 (DONE, this lane):
`frontends/gpui/tests/undo_survives_blur_windowed.rs` — real click, real
keystrokes, real Cmd-Z, real blur click, store-level oracle, three arms
(SqlOnly, SqlOnly-after-row-set-rebuild, Loro control), each asserting the
mode it booted; mutation-proven (disabling the `arm()` call at
`share_ui.rs:915` reds all three). Missing piece, part 2 (NOT done, filed):
the headless mirror gains a DELIVERABLE/ORPHANABLE echo model (so "this
editor received no data-sync" is representable) plus a blur-commit
transition, which is what would let the ONE keystone PBT reach this class at
all. ENVIRONMENT (secondary): the rung is GREEN at main in all three arms,
so the condition that defeats the re-seed live is not reproducible in the
harness — window activation state is not modelled, and the dogfood could not
even verify visually (F6).

## Remedy

OPEN — pinned, not fixed. The rung is committed and green; the
live-condition reproduction is NOT achieved and the lane says so rather than
claiming the class closed. Fix direction, in order: (1) close the DISCLOSURE
hole first, because it is what makes this undebuggable in a live session —
`reseed_gesture.arm()` (`frontends/gpui/src/share_ui.rs:915`) DISCARDS its
`ReseedArm` result, and of the four outcomes only `LocalEditRaced` (warn)
and `FocusMoved` (debug) log anything: `Armed` and `NoTargetRow` are silent,
and the render-side apply (`editable_text.rs:160-176`) logs nothing either,
so a dogfooder cannot tell "refused" from "never applied". The enum was
built so a silently skipped re-seed is unrepresentable; the call site
defeats that. (2) With that in place, re-run the dogfood shape and read
which outcome fires. (3) Only then extend the re-seed's coverage.
ADJACENT/OVERLAP, reported not touched: if the live revert turns out to come
from the org write-back / re-ingest cycle rather than the blur commit, this
row's class merges with F2 and the fix belongs to the concurrently-running
orgmode-writeback lane, not here.
