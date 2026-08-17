---
id: 2026-08-17-hidden-slash-command-restore-anchor-goes-stale
date: 2026-08-17
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Backspacing out of a slash-command picker typed anywhere but column 0 sliced
  the buffer with a hide-time offset that no longer indexed it.
---

## Bug

Found by the fresh-context verifier reviewing lane `lane-popup-ux` (tasks #45
and #47), not by a test — evidence in `lane-popup-ux-verify.md`. Ruling D1.b
hides a slash command's typed text while its picker phase is open and restores
it on cancel. The restore anchor was the `prefix_start` captured when the text
was hidden, and nothing revalidated it against the buffer it was later used to
slice.

Block content `hello /emb` (a mid-line command, which the trigger accepts with
`prefix_start = 6`):

1. Enter opens the picker; the buffer becomes `hello ` with the caret at 6.
2. Backspace. The caret is non-zero so `InputState` performs a plain character
   delete; the buffer becomes `hello`, 5 bytes.
3. `on_text_changed` sees `cursor_byte (5) < prefix_start (6)` and asks for a
   restore at offset 6.
4. The restore evaluates `&text[..6]` against a 5-byte string.

Three further defects of the same family surfaced in the same pass:

- The hide span was `abs_start..cursor`, so pressing Left before picking hid
  only `/em` and left `b` in the block.
- Popup row clicks dispatched by render-pass INDEX. Items refill
  asynchronously, so a stale but in-range index ran a different command than
  the row the user saw.
- The one-shot that suppressed the restore's own change event assumed
  `set_value` fires exactly one event; on zero it stayed armed and swallowed
  the user's next real trigger.

## Root cause

One mechanism in four places: **a coordinate captured at one instant, reused at
another without revalidating that it still describes the present.** The hide
anchor was an offset into a buffer that kept changing; the hide span was
derived from a caret that had since moved; the click carried an index into a
list that had since been refilled; the suppression flag asserted a future event
count.

The reachable route was also mis-analysed. The backspace-out branch is
unreachable at `prefix_start == 0` — backspace at caret 0 is intercepted as
`join_block` — so the ONLY way into it was `prefix_start > 0`, precisely the
case that could not slice. And the branch never restored anything anyway: the
per-keystroke change handler routed its action through `execute_action`, whose
no-window arm drops `RestoreCommandText` on its `_ => {}` catch-all.

## Missing piece

Every rung written for D1.b typed its command at column 0 and none pressed
Backspace, so `prefix_start` was always 0 and the anchor could never go stale.
The interaction was generatable in the windowed harness; it simply was not
generated. That also made the restore assertions vacuous — they passed with the
hiding reverted, because a restore that no-ops leaves the same buffer a
never-hidden command does.

The keystone cannot cover this class at all: the headless mirror calls the
provider directly and never advances into a picker phase
(`crates/holon-frontend/src/headless_editor_mirror.rs`), so `PhaseAdvanced`
carries no hide span there and none of this code runs. Reaching it headlessly
would need the mirror to drive `PopupMenu` phases the way the frontends do,
rather than short-circuiting to the provider — the prod/test parity work this
escape argues for.

## Remedy

Fixed in `lane-popup-ux` round 2. The anchor is no longer an offset:

- `crates/holon-frontend/src/editor_view_model.rs` — `HiddenCommandText` stores
  `line_prefix: String`, the bytes that stood before the hidden command.
  `anchor_in(line)` returns an offset only while the live line still starts
  with exactly those bytes, so an anchor cannot be used without being
  revalidated. `on_text_changed` is total: it feeds the search term while the
  prefix holds, and otherwise ends the phase with a `tracing::warn!` rather
  than reinserting the command where the user never typed it.
- `frontends/gpui/src/views/editor_view.rs` — a backspace at the anchor is
  intercepted and CANCELS the phase with a proper restore, so the only
  reachable exit is the restoring one; the change handler now routes through
  the shared `apply_popup_action` instead of `execute_action`'s no-window arm;
  and both text arms bound their spans to the line via `line_bounds`,
  disclosing and skipping rather than slicing when a span does not fit.
- `crates/holon-frontend/src/popup_menu.rs` — `PopupResult::PhaseAdvanced`
  carries a `HideSpan` whose `len` is the trigger length plus the filter the
  MENU matched on, never a caret-derived span; `select_index` takes the
  `expected_id` the row painted and drops the click when the index no longer
  holds it.
- `restoring_to: Option<String>` holds the line the restore will produce, so
  the suppression matches on content and disarms itself if the programmatic
  change event never arrives.

Oracles added — `frontends/gpui/tests/slash_command_text_hidden_windowed.rs`:

- `backspacing_out_of_a_mid_line_picker_cancels_and_restores` (the keystone
  rung, and the only non-vacuous restore rung: with hiding reverted the
  backspace eats the `b` and the buffer reads `hello /em`),
- `hiding_covers_the_whole_command_even_with_the_caret_moved_left`,
- `a_cancelled_picker_leaves_the_trigger_working`, which asserts MENU STATE via
  `EditorView::is_popup_active` — the bounds registry keeps the last frame that
  painted rows and reports a closed menu as still open.

And `crates/holon-frontend/src/popup_menu.rs`:
`click_on_a_row_the_list_replaced_is_ignored`, red with the index-only
dispatch as `Execute { op_name: "set_field" }` for a click on `embed`.

Red logs: `/tmp/lane-popup-ux-r2-red.log`, `/tmp/lane-popup-ux-r2-click-red.log`.
