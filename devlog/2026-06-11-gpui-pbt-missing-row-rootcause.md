# 2026-06-11 — gpui PBT "missing row" ROOT-CAUSED & FIXED: virtualized main panel + broken scroll-to-reveal + occluded-window frame starvation

Continues `2026-06-10-gpui-pbt-settle-followups.md`. The handoff left the
bug localized to "paint side" (row in block_raw + matview + watch rows +
HeadlessLiveTree, absent only from BoundsRegistry). This session found
TWO stacked mechanisms and fixed both; `gpui_ui_pbt_no_loro` is green on
the previously-failing persisted regressions.

## Investigation path (dead ends kept — they carry the probes)

1. **Paused replay** (`PBT_PAUSE_SECONDS=1800`): reproduced on the persisted
   regression. NOTE: the windowed replay harness (`windowed_replay.rs`) does
   NOT start the embedded MCP — the port-8528 workflow only exists in the
   old `run_in_gpui_window` path. Inspection: `lldb -p`, CGWindowList,
   `screencapture -l<win>`.
2. The test window runs `onscreen:false` (background job → covered/other
   Space); main thread idle in the AppKit runloop; no deadlock.
3. **Frame-generation probe** (new, permanent): `wait_for_entity_bounds`'s
   timeout dump now prints "N frame(s) committed during the wait" vs "NO
   frame committed". First instrumented failure: *53 frames committed,
   element truly absent* → pure frame starvation refuted as sole cause.
4. **Render probe** (`HOLON_GPUI_RENDER_PROBE=1`, permanent, env-gated,
   prints per-shell item-count changes): the main panel renders through a
   **list-mode `ReactiveShell`** (`gpui::list`, measured virtualization) —
   the `scroll_entity_into_view` doc claiming "Main panel doesn't
   virtualize" was stale. At failure the shell held items=26 visible=26
   INCLUDING the target — second-to-last row, below the 948 px viewport.

## Root causes (two, stacked)

**A. Broken scroll-to-reveal lookup (the deterministic half).**
`scroll_entity_into_view` looked up the list shell by
`CacheKey::ReactiveShell(view.stable_cache_key())` where `view` came from a
**fresh** `engine.snapshot_reactive()` — fresh snapshots build new
`ReactiveView` instances → new keys → the lookup NEVER matched the rendered
shell. Log signature: `found (view ix=24) but no ReactiveShell for key
<hash> in panel cache`. Every wait for a row below the virtualized viewport
therefore timed out; "four faces" = whichever wait (SplitBlock bounds /
widget-kind / window-focused-editor / children-settled) hit an off-viewport
row first. `wait_for_children_settled` already had the correct
accumulate-across-scroll-positions design — sitting on the broken scroll.

**B. Occluded-window frame starvation (the flaky half).**
With the window occluded, frames commit only via the one
`window.refresh()` forced after each synthetic input. Async follow-ups
(editable-variant swap after click-to-focus, post-split row insert) land
*after* that refresh; their `cx.notify()` schedules nothing on an occluded
window, so the committed BoundsRegistry freezes for the whole wait.
Whether a step passed = whether the async update won the race against the
input's single refresh → the per-run face roulette.

**Trap discovered while fixing B:** a global 100 ms frame pump fixes the
starvation but BREAKS per-keystroke pacing — typing paces on "one
committed frame per key", pump frames satisfy that wait before the editor
echo lands, dropping characters (inv-displayed-text `"#ir"` vs `"#+ir"`).
Pump must therefore live *inside the waits*, never globally. Waits and
typing never overlap (transitions are sequential), so pumping per retry is
safe.

## Fixes (all landed in this worktree)

- `frontends/gpui/src/lib.rs` — `scroll_entity_into_view` rewritten: walk
  the panel shell's `entity_cache` for cached `ReactiveShell`s and query
  each via new `ReactiveShell::visible_index_of(uri)` (also fixes the
  raw-vs-visible index bug under tree-collapse — `scroll_to_reveal_item`
  takes visible-row coordinates). `find_collection_for_entity` deleted.
  The `ScrollEntityIntoView` interaction branch now `window.refresh()`es
  unconditionally — this is what lets the RPC double as a frame pump.
- `crates/holon-integration-tests/src/pbt/sut.rs` —
  - `wait_for_entity_bounds`: scroll re-armed every 300 ms (reveal + pump),
    plus the frame-generation diagnosis in the timeout dump.
  - `wait_for_widget_kind` / `wait_for_window_focused_editor`: scroll RPC
    dispatched on every unsuccessful retry (reveal-if-off-viewport + pump).
- `frontends/gpui/src/user_driver.rs` —
  - `send_raw_keystroke`: pre-reveals the focused row when it has no
    committed bounds (virtualized viewport may have scrolled it out; its
    editor can't consume keys while unmounted).
  - `scroll_to_entity` no longer swallows the response `detail` (fail-loud).
  - New `send_raw_keystroke_until_handled` override →
    `key_down_until_handled` (which now also scrolls the focused entity
    into view between retries).
- `crates/holon-frontend/src/user_driver.rs` — trait gains
  `send_raw_keystroke_until_handled` (default = single-shot forward;
  headless drivers consume synchronously).
- `crates/holon-integration-tests/src/pbt/sut_handle.rs` — ArrowNavigate
  uses `send_raw_keystroke_until_handled` (2 s): after a focus-moving op
  the consuming editor may mount on a later pass.
- `crates/holon-frontend/src/geometry.rs` + `frontends/gpui/src/geometry.rs`
  — `GeometryProvider::generation()` (default 0; gpui →
  `committed_generation()`).
- `frontends/gpui/src/views/reactive_shell.rs` — `visible_index_of` +
  env-gated `HOLON_GPUI_RENDER_PROBE=1` change-only item-count probe.
- `frontends/gpui/tests/pbt_harness/{windowed_replay,mod}.rs` — explicit
  do-NOT-add-a-global-frame-pump comments (with the dropped-character
  reason).

## Validation

- `just pbt gpui_ui_pbt_no_loro 40 1` (regressions replay first): GREEN
  ("all 1 case(s) passed", exit 0) after the full fix set; repeats + the
  `gpui_ui_pbt` Loro twin in flight at time of writing — see session log.
- Failure-point progression during the fix sequence (each fix moved the
  failure deeper, all faces accounted for): SplitBlock bounds (step 36) →
  ArrowNavigate unconsumed (step 38) → widget-kind (new case) →
  window-focused-editor → typing-pacing regression from the global pump
  (reverted) → GREEN.

## Traps for future sessions

- `jj st` at session start auto-resolved a concurrent-op divergence and
  moved the working copy to a FRESH EMPTY commit off main — `jj new
  vpvvuums` restored the worktree. After any "concurrent modification"
  message, check `@`'s parent before trusting the working tree.
- Killing a batch `lldb -p` attach can take the inferior down with it
  (zombie) — rerun the repro rather than reusing it.
- The windowed harness sets `PBT_PAUSE_SECONDS=0` by default; an explicit
  env value overrides it (panic pause works), but there is NO MCP on 8528
  in this harness path.
- Global frame pumps break keystroke pacing (dropped characters). Pump
  inside waits only.
