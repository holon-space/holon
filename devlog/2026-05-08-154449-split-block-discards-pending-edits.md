# Production `split_block` silently discards pending in-memory edits

**Date**: 2026-05-08
**Continues**: `devlog/2026-05-08-152913-inv-displayed-text-active-editor-fix-bugs-surfaced.md`
**Status**: surfaced (production bug); model is correct, panic should keep firing.

## What I did this turn

1. Followed up on user's hint that "split_block should right-trim the original and left-trim the new block contents." Verified the contract is implemented on both sides:
   - Production: `crates/holon-core/src/traits.rs:706` (`split_block`) — `content_before.trim_end()` at L735, `content_after.trim_start()` at L738.
   - Ref-state: `crates/holon-integration-tests/src/pbt/reference_state.rs:1379-1380` — same `trim_end` / `trim_start`.
2. Identified a genuine ref-state model gap: `PressKey::apply_to_ref` had no handler for `Backspace` at `cursor > 0` (the "mid-line backspace" path). Production's `InputState` removes one character; ref-state did nothing → `inv-displayed-text` saw on-screen one char shorter than `in_memory_content`. Added the handler:
   ```rust
   else if matches!(single, Some(Key::Backspace)) && !has_modifier && cursor_byte > 0 {
       if let Some(editor) = state.active_editor.as_mut() {
           editor.delete_backward(1);
       }
   }
   ```
3. Re-ran seed 5. The original off-by-one panic is gone. A new panic surfaced — **a different bug**, also worth keeping.

## The new seed 5 finding: production discards pending in-memory edits on Enter

`assertions.rs:60` `Backend diverged from reference: Blocks differ`:

| Block | Production | Ref-state |
|---|---|---|
| `block:----p7-s---t41v-el-z4` (original) | `content: "PoL1 V O"` | `content: "PoL1 V O"` ✓ |
| split-suffix block | `content: "16"` (real UUID) | `content: ""` (synthetic id) |

Both sides agree on the original block's content (right-trim worked). They disagree on the new (suffix) block:
- **Production** read `block.content` from DB at split time, which still held the **initial** `"PoL1 V O   16"` (because MutableText doesn't sync to `block.content` per keystroke — verified two devlogs back). Splitting at the cursor byte (≈11) over that string yields prefix `"PoL1 V O"` + suffix `"16"`.
- **Ref-state** committed `in_memory_content` (`"PoL1 V O   "` — the user's typed-down state) into `block.content` before splitting (per `press_key.rs:125`). Splitting that at the same cursor yields prefix `"PoL1 V O"` + suffix `""`.

So production silently discards the user's pending typing when they press Enter. The user types into a block, presses Enter expecting their typing to be in either the prefix or the new suffix — instead, the split happens against the pre-edit state and the typed-but-uncommitted text vanishes.

This is a real production bug that the PBT now exposes. The right fix is in production: `split_block`'s Enter handler in `frontends/gpui/src/views/editor_view.rs:548-572` should commit pending `InputState` text via `set_field` before dispatching `split_block`, or the operation should accept the live text as a parameter rather than re-reading from DB.

## What I did NOT do

- Did NOT modify ref-state's `split_block` to mirror production's "discard pending edits" behavior. That would silence the signal. The user's principle this conversation: PBTs lift prod and test code — ref-state encoding the *intended* behavior (commit then split) and panicking against the buggy production is exactly the value PBTs add.
- Did NOT add a special case to `inv-displayed-text` to skip post-split blocks. Same reason.

## Files

- `crates/holon-integration-tests/src/pbt/transitions/press_key.rs` — added `Backspace` at `cursor > 0` handling in `apply_to_ref`.

## Bugs the PBT now flags (cumulative across this thread)

1. **Editor `InputState` displays the wrong content after `split_block`** (split-block UI staleness — pre-existing per inv-displayed-text comment; PBT now reaches it under boosted weights).
2. **`split_block` discards pending in-memory edits** (this devlog) — pressing Enter mid-typing loses the typed text.
3. **`current_focus` matview missing row after `NavigateFocus`** (seed 8) — IVM lag or missing INSERT.
4. **Turso scheduler timeout for 'blocks' table** (seed 6) — `mark_available()` never called.

## Next investigation

A natural next step is to trace why production's `split_block` reads stale `block.content`. The most-direct fix would be the Enter handler at `editor_view.rs:548` committing the InputState text via `set_field` before dispatching `split_block`, but the cleaner fix is at the operation level: have `split_block` accept the live content (or a `pending_text` param) so the storage layer doesn't depend on per-keystroke commits.
