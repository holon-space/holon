# `inv-displayed-text` invariant fixed; real cursor/split bugs now surfacing

**Date**: 2026-05-08
**Continues**: `devlog/2026-05-08-142643-pbt-verifies-blur-on-nav-empirically.md`

## Setup

Boosted weights to force edit-path coverage:

```
HOLON_PBT_WEIGHTS='FocusEditableText:30,TypeChars:30,MoveCursor:30,PressKey:30,DeleteBackward:20'
```

Ran 8 seeds at this weighting. Iterated on what the PBT surfaced.

## What I tried first (and got wrong)

The `inv-displayed-text` invariant compared the rendered text against `block.content_text()`. Seed 5 panicked: on-screen had a typed-but-uncommitted `n`, ref's `block.content` did not. Comment at `sut.rs:5991` claimed MutableText commits keystrokes synchronously, so I patched `TypeChars::apply_to_ref` to call `commit_active_editor_if_changed()`.

Re-ran seeds. The "Backend diverged from reference: Blocks differ" assertion (`assertions.rs:60`) fired with the inverse signal:

- **SUT**: `block.content = "PoL1 V O   16"` (no n — production did NOT sync)
- **ref**: `block.content = "PoL1 V O   16n"` (my pre-bake wrote the n)

So the comment was wrong, my patch was wrong, and the empirical truth is:

> **MutableText keystrokes update the editor's live state but do NOT synchronously commit to `block.content`. The on-screen string reflects the live `InputState`; `block.content` lags until blur / Enter / chord-commit.**

This is the bug-class the user flagged: cursor navigation + split_block both rely on the live `InputState`, not on `block.content`, so they diverge from any model that pre-bakes typing into storage.

## What I changed

1. **Reverted** the bogus `commit_active_editor_if_changed()` in `TypeChars::apply_to_ref` and `DeleteBackward::apply_to_ref`.
2. **Fixed `inv-displayed-text`** at `crates/holon-integration-tests/src/pbt/sut.rs` to compare on-screen against `active_editor.in_memory_content` when an editor is active on the block, falling back to `block.content_text()` otherwise:
   ```rust
   let expected: String = match &ref_state.active_editor {
       Some(active) if active.block_id == block.id => {
           active.in_memory_content.clone()
       }
       _ => block.content_text().to_string(),
   };
   ```
3. The misleading comment at `sut.rs:5991` ("InputState and SQL stay in sync even while the user is typing") is now contradicted by the new dispatch — this devlog supersedes it. Comment cleanup pending or it'll mislead the next investigator.

## What the PBT now surfaces (boosted weights, seeds 1-8)

| Seed | Result | Class |
|---|---|---|
| 1, 2, 3, 4, 7 | pass | — |
| 5 | `inv-displayed-text` after `FocusEditableText → TypeChars → DeleteBackward → PressKey-Enter`: on-screen `"PoL1 V O "` (9 chars) vs expected `"PoL1 V O  "` (10 chars). **Split_block stale prefix bug** — the PBT correctly identifies that the InputState/render didn't pick up the post-split content. User flagged this. | Production UI staleness post-split |
| 6 | `SCHEDULER BUG: query_and_watch timed out … mark_available() never called for 'blocks' table` | Independent — Turso scheduler |
| 8 | Navigation focus mismatch — `current_focus` matview row missing for `block:ref-doc-0` after `NavigateFocus` | Independent — `current_focus`/`navigation_history` propagation |

## Files

- `crates/holon-integration-tests/src/pbt/transitions/type_chars.rs` — reverted apply_to_ref change; comment explains why we deliberately don't commit
- `crates/holon-integration-tests/src/pbt/transitions/delete_backward.rs` — same revert + comment
- `crates/holon-integration-tests/src/pbt/sut.rs` — inv-displayed-text now compares against `active_editor.in_memory_content` when applicable
- Drive-by: bare `_` for unused trait params in 3 files (archlint)

## Lesson from this turn

The misleading code comment ("MutableText commits synchronously to SQL") is exactly the kind of unverified assumption the user warned against last turn. I trusted it instead of running the experiment. The PBT delivered the right error message anyway — first as `inv-displayed-text`, then as `Backend diverged` once I made the wrong "fix" — and both panics together pinned down the real contract.

Reading panics in pairs (model says X, prod says Y) is more informative than either single panic. I'll lean on this pattern.

## Open follow-ups

1. **Split_block staleness on the InputState** (seed 5): real production bug. Tracing would need: what does GPUI's editor view do on `OperationIntent::split_block`? Does it update its bound `InputState` to reflect the new (shortened) content? Suspected: the InputState attached to the original row keeps the full pre-split text after the structural mutation.
2. **Navigation focus mismatch** (seed 8): `current_focus` matview returns no row for the navigated-to block. Could be IVM lag or a missing `navigation_history` insert in the SUT path. Independent of the editor work.
3. **Turso scheduler timeout** (seed 6): `mark_available()` never called for 'blocks'. Independent.
4. **MoveCursor coverage**: the user explicitly flagged it but it didn't fire in any of these 8 seeds. May need an even-higher weight (`MoveCursor:80`) or a TypeChars→MoveCursor sequence-based generator to expose its bugs.
5. **Clean up the misleading `sut.rs:5991` comment** before another investigator trusts it.
