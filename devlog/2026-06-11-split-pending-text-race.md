---
date: "2026-06-11 09:20"
session: "ee514541"
project: "gpui-pbt-speed"
---

# The "typing contamination" family, root-caused twice

The flaky gpui PBT failures previously classified as user-activity blur
contamination ("#ir" vs "#+ir" content divergence, split focus handoff
diverged, block count mismatch after split, slash-popup filter chars lost)
reproduce idle-clean and decompose into TWO real causes — neither is user
blur, neither is dropped keystrokes.

## Cause 1 (harness, FIXED): single-sample race against the async focus handoff

`split_block`/`join_block` apply their focus+caret result in the spawned
dispatch task (`apply_structural_focus`, ADR 0010). `wait_for_blocks_synced`
converging does NOT imply that task has run. The `[PressKey-Enter]` check
sampled `engine_focused_block()` once and raced it — red exactly when the
window was key/active because the busier main thread widens the race window
(this is the entire "window-active correlation" that masqueraded as user-blur
contamination). Fix in `sut_handle.rs`: poll until convergence, 2s deadline,
fail loud. Validated: that face disappeared from probe batches after the fix.

## Cause 2 (PROD BUG, OPEN): structural ops use backend content + editor cursor

Smoking gun (probe11_run1):

    Operation block.split_block failed: Split position 8 exceeds content length 3

The editor held `"ßñ😀中797"` (14 bytes; typed chars PENDING — SqlOnly
commits on blur), the backend still held `"797"` (3 bytes). The Enter
capture_action dispatches `split_block` with the EDITOR's cursor byte, but
the op computes the split against the BACKEND's content. Any real user who
types into a block and presses Enter without an intervening blur hits this:
the op either fails loudly (position out of range) or splits stale content,
silently losing/misplacing the pending text. Flakiness = whether focus churn
happened to fire an on-blur commit before the split.

Fix options (user decision):
1. **Atomic content+split**: the Enter/Backspace-at-0 capture handlers pass
   the live editor text in the intent (`params.content = editor_text`); the
   backend op writes it before splitting. No ordering hazard; op signature
   gains an optional param.
2. **Commit-before-dispatch**: flush pending text (the on_blur path) before
   dispatching the structural intent. Needs ordering guarantees — both ops
   are fire-and-forget spawns today, so this requires a chained dispatch
   (single spawned task awaiting commit then split), not two dispatch_intent
   calls.

The ref model already treats structural ops as commit-points
(`commit_active_editor_if_changed` before joins), so option 1/2 also
converges ref and prod semantics.

## Instruments added (worktree change, env-gated/log-only)

- `[interaction-pump] CONTAMINATION` marker + response detail when the test
  window is not key/active at dispatch; `InteractionEvent` gains `Debug`.
- `key_down_until_handled` logs keystrokes consumed only after a retry.
- `HOLON_GPUI_CARET_PROBE=1`: `[caret-seed]` (every grab/seed decision),
  `[focus-promote]` (authority steals — 0 hits in red runs, refuting the
  steal-back hypothesis), `[split-dispatch]` (cursor+live text at every
  structural dispatch), `[data-sync]` (skip/apply decisions).
- `HOLON_GPUI_FORCE_ACTIVE=1`: activate the window at startup (repro lever;
  does not always stick).

## Open

- `wait_for_entity_bounds` timeout where the failure dump (ms later) shows
  the element WITH bounds (probe11_run4) — late render or staged→committed
  promotion gap; needs its own probe round.
- DeleteBackward's per-keystroke gate reads engine focus that may not have
  moved yet after a join (same shape as Cause 1); fix sketch: scratch-clone
  pre_ref_state, run delete_backward_apply_to_ref count=1 per keystroke,
  wait for engine focus == expected.
- Idle-gated validation method: judge runs by `end-idle ≥ wall`; scripts in
  `~/.claude/jobs/ee514541/tmp/idle_gate_*.sh`.
