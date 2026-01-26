# gpui_ui_pbt reactive settle — per-transition cost vs general_e2e_pbt

Goal: per-transition wall time of `gpui_ui_pbt` (real window) within a low
constant factor of `general_e2e_pbt` (headless ReactiveViewModel), replacing
polling/fixed sleeps with reactive (event-driven) settle detection.

## Measurement harness

`HOLON_PBT_STEP_TIMING=1` prints `[step_timing] step=N <Variant>
apply_ms=… check_ms=…` from both loop engines:

- `pbt::stepper::run_sequence` (headless slices)
- `pbt::fixtures::replay_steps` (gpui windowed replay)

Summarize with `awk` per variant (`summarize_step_timing.sh`). NEVER measure
with a concurrent build/test running (~4× inflation, known from earlier
perf work).

## Baseline (SqlOnly, runs in isolation, 2026-06-10)

| metric | headless | gpui | factor |
|---|---|---|---|
| ALL avg/step | 246 ms | 340 ms | 1.38× |
| SplitBlock | 257 ms | 421 ms | 1.64× |
| NavigateFocus | 219 ms | 208 ms | ~1.0× |
| check phase | 94 ms | 117 ms | 1.24× |

The shared per-transition floor (CDC quiescence + invariant settle, ~110 ms
check + ~120 ms apply-side) dominates BOTH harnesses. The gpui-only delta
lived in driver-verb apply paths: fixed sleeps (15–30 ms after every
synthetic input event, ×N for keystrokes) and 10–50 ms interval polls on
BoundsRegistry / focus.

## Changes

1. **BoundsRegistry frame-commit notify** (`frontends/gpui/src/geometry.rs`):
   `tokio::sync::Notify` fired on every committed-buffer rotation
   (`begin_pass`/`flush`) and cold-phase `record`.
   `GeometryProvider::changed()` (new default trait method, 20 ms tick
   fallback) exposes it; waiters wrap in a 50 ms `timeout` so a wake landing
   between predicate check and await degrades to a slow poll, never a hang
   (GPUI only paints on demand).
2. **Geometry waits wake per frame commit**: `wait_for_entity_bounds`,
   `wait_for_widget_kind` (via new `retry_until_ok_wake`),
   `wait_for_children_settled`.
3. **Focus wait is signal-driven**: `wait_for_focus_to_match` consumes
   `UiState::focused_block_mutable().signal_cloned().to_stream()` —
   futures-signals emits the current value first, so there is no
   check-then-wait race and zero polling.
4. **All fixed sleeps removed from `GpuiUserDriver`.** The interaction pump
   dispatches each `PlatformInput` synchronously inside `cx.update_window`
   before acking, so ordering needs no pacing. The real async gap — engine
   focus moves synchronously on click but the editor that consumes keys
   mounts on the NEXT render pass — is now synchronized semantically:
   - `send_key_chord`: signal-driven wait for `focused_block == target`
     after the focusing click; `key_down_until_handled` retries an
     unconsumed key (no side effects) waking per frame commit, fail-loud
     after 2 s.
   - SplitBlock pipeline: explicit `wait_for_widget_kind(id,
     ["editable_text"])` after click — the mount that grabs window focus has
     run once the editable variant is committed.
5. **Windowed replay paint quiescence**: 240 ms element-count stability
   window sampled at 30 ms (was 4×120 ms ≈ 0.5 s floor) per candidate.

## After (SqlOnly, passing runs)

| metric | headless | gpui | factor |
|---|---|---|---|
| ALL avg/step | 246 ms | ~311 ms | **1.26×** |
| SplitBlock | 257 ms | ~403 ms (success path ~390; replayed fixture applies 500–640 → 240–260) | ~1.5× |

Remaining gpui-only cost is genuine UI latency (editor mount + double-buffer
promotion need real frames) plus geometry-reading invariants in check —
not polling.

## Second wave: races the speedup exposed (and their structural fixes)

Removing the sleeps surfaced a family of REAL synchronization gaps that the
old pacing had been masking statistically. The unifying mechanism: **engine
`focused_block` moves synchronously, but WINDOW focus follows a spawned
binding / editor mount one or more frames later** — and the pump's
`handled` flag cannot distinguish "consumed by the right editor" from
"consumed by the previously-focused one".

Fixes (all reactive, no fixed sleeps):

1. `ElementInfo.focused: Option<bool>` — the `editable_text` builder records
   `input.focus_handle(cx).is_focused(window)` at render time. Window focus
   is now OBSERVABLE per committed frame.
2. `SutLayout::wait_for_window_focused_editor` — gates every
   keystroke-bearing path: SplitBlock pipeline after click,
   `send_key_chord` after its focusing click, `sync_caret_to_new_split_block`
   (post-split `home`), and TypeChars/DeleteBackward/MoveCursor entry
   (pre-state active editor).
3. Per-keystroke re-gate inside `DeleteBackward`: a backspace at offset 0
   JOINS blocks and moves focus to the merged block — later backspaces must
   wait for the merged editor's window focus (re-reads the engine's current
   `focused_block` per retry).
4. Keystroke frame pacing: `send_raw_keystroke` waits one committed frame
   after a consumed key (wake-on-commit, 50 ms cap). Sub-frame typing
   outruns the editor's focus-gated backend-echo handling and DROPS typed
   characters (`shown: "bHOqb"` vs `expected: "bxHOqb"`) — no human
   keystroke outruns the compositor, so this is fidelity, not slowdown.
5. The interaction pump calls `window.refresh()` after every synthetic
   input, so commit notifications fire even for an occluded window
   (otherwise every wait degrades to its timeout cap — observed as a 3×
   slowdown when the test window was hidden).

## Prod bugs surfaced AND fixed (byte/char conflation family)

Full-mode runs kept crashing the main thread on multibyte content; both are
prod bugs in the link/slash trigger pipeline (same family as the old
`compute_text_delta` overflow):

1. `editor_view.rs` passed `cursor_position().character` (a CHAR column)
   into `on_text_changed` → `check_triggers`, which slices the line by BYTE
   offset → panic inside 'ß'. Fix: convert char column → byte offset at the
   GPUI boundary; `check_triggers`/`on_text_changed` params renamed to
   `cursor_byte` and documented as fail-loud on non-boundaries.
2. `view_event_handler.rs` command_menu update path used
   `current_line[1..]` ("text after /") — wrong since the slash trigger
   fires MID-line, and a byte-slice panic on a multibyte first char ('😀').
   Fix: use `filter_text` (text between the matched prefix and the cursor),
   matching the doc_link arm and the activate call.

## Pre-existing issues surfaced (NOT fixed here)

- `inv-blocks-match-ref/org` fails after `Indent` under `PBT_NO_LORO=1`
  (org render divergence; warn-gated via
  `HOLON_PBT_INVARIANTS='inv-blocks-match-ref/org:warn'` for timing runs).
  Regression persisted at
  `crates/holon-integration-tests/tests/gpui_ui_pbt.proptest-regressions`.
- Whitespace-only blocks (content `"\t"`) never render a
  `rendered_text`/`editable_text` widget → `wait_for_children_settled`
  deterministically times out (5 s) when such a block is among expected
  siblings; the candidate aborts off-signature and is swallowed (run still
  "passes"). Also occasional wrong-focus click aborts (known hit-test /
  bounds-shift family). Both classes present in baseline at similar rates.
- A panicking candidate whose message lacks "trouble begins at:" makes the
  whole run report PASS (off-signature swallow happens even for the FIRST
  failure, not just shrink candidates) — and the gpui_ui_pbt process exits 0
  even after a final shrunk-failure panic. Exit codes are not trustworthy;
  grep for `all N case(s) passed`.
