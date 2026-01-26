# Handoff: gpui_ui_pbt does not exercise `split_block` (or any edit transition)

**Date**: 2026-05-08
**Status**: Root cause confirmed via DAP debugger session. Fix not yet implemented.
**Why this exists**: User asked "does this PBT detect bugs we still have, e.g. `split_block`?" — answer is **no, the test cannot reach those bugs in its current configuration**.

---

## Question

> Run `frontends/gpui/tests/gpui_ui_pbt.rs`. Does it detect `split_block` bugs?

## Short answer

**No.** Across a full 50-step run (`passed: 47/50`, exit 0, no panics), zero edit transitions fired:
- `SplitBlock`, `JoinBlock`, `ClickBlock`, `FocusEditableText`, `TypeChars`, `MoveCursor`, `DeleteBackward`, `EditViaViewModel`, `Indent`, `Outdent` — all 0 occurrences.
- 3 of 50 steps were generator returns of `None`; the other 47 were setup/navigation/sync (`EmitMcpData` ×7, `SyncWithPeer` ×5, `SwitchView` ×5, `NavigateHome` ×4, `Nothing` ×4, `CreateDocument` ×3, `AddPeer` ×3, `SetupWatch` ×2, `NavigateForward` ×2, `NavigateBack` ×2, `MergeFromPeer` ×2, `CreateDirectory` ×2, `ConcurrentSchemaInit` ×2, plus 1× each of `WriteOrgFile`, `StartApp`, `SimulateRestart`, `PeerEdit`).

Run command:
```
cargo test -p holon-gpui --test gpui_ui_pbt --features pbt 2>&1 | tee /tmp/gpui_ui_pbt2.log
```
Note: this test is `harness = false`. **Don't use `cargo nextest`** — it can't enumerate the binary and fails with `line "..." did not end with the string ": test"`.

## Root cause (debugger-confirmed)

The starvation chain is in `crates/holon-integration-tests/src/pbt/reference_state.rs`:

- `main_editable_descendants()` at L1187 returns `Vec<EntityUri>` of size **0** every time it's called post-startup.
- It calls `expected_focus_root_ids(Region::Main)` at L1564, which **does** return a non-empty BTreeSet (size = 1).
- The single pinned block in Main is the layout block `block:default-main-panel`.
  - This block IS in `layout_blocks`, so the filter at L1197 (`!self.layout_blocks.contains(id)`) excludes it.
  - Its descendants (in the inv14 TREE log: `vms_button::block:default-main-panel::table_view`, `…::board_view`, `…::tree_view`, `…::source`) are layout/UI blocks, **not `Text` non-page user content**, so the `b.content_type == ContentType::Text && !b.is_page()` filter at L1195/L1196 drops them.

Result: every edit-path generator that filters `state.main_editable_descendants()` (`SplitBlock`, `Indent`, `Outdent`, `EditViaViewModel`, etc.) and every per-region call (`ClickBlock` for Main, gated by `region_predictable` + `focusable_rendered_block_ids`) returns `None`. **The PBT can never reach editing code paths in this configuration.**

This matches the long-standing memory note `gpui_pbt_edit_transitions_starvation.md`.

## Concrete debugger evidence

Built with `cargo test --profile debugger -p holon-gpui --test gpui_ui_pbt --features pbt --no-run`. Binary: `target/debugger/deps/gpui_ui_pbt-8cd4cab601d5375a` (size matters: dev profile uses `debug = "line-tables-only"`, no locals).

| Stop point | Frame | Local | Value |
|---|---|---|---|
| `live_geometry.rs:83` (filter_to_rendered) — called from `ClickBlock::weighted_generator @ click_block.rs:50` | top | `blocks: Vec<EntityUri>` | **size=0** |
| same | top | `rendered: HashSet<String>` | size=4 (the 4 sidebar items with `has_content=true`) |
| same | parent (click_block.rs:50) | `region: Region` | `Main` |
| same | parent | `main_unfocused: bool` | `true` |
| same | parent | `arms: Vec<...>` | size=0 |
| `indent.rs:40` (after `let indentable = …`) | top | `indentable: Vec<EntityUri>` | **size=0** |
| `split_block.rs:43` (after `let editable_block_ids = …`) | top | `editable_block_ids: Vec<EntityUri>` | **size=0** |
| `reference_state.rs:1568` (inside `expected_focus_root_ids` closure) | parent | `region: Region` | `Main` |
| same | top | `pins: Vec<OpenPinEntry>` | **size=1** |
| same | drilled into `pins[0]` | `history_id: i64`, `added_ts_logical: u64` | both `1` |
| same | `pins[0].block_id` | `Option<EntityUri>` | `Some(<block: scheme URI>)` (visualizer can't print the val String — niche-optimised, memory addresses showed up as integers; deduced by cross-referencing the inv14 TREE log) |

## Caveats from this session

1. **LLDB / CodeLLDB auto-expansion crashes the adapter at >4GB RSS.** Hit this twice trying to expand `ReferenceState` and `BTreeSet<EntityUri>`. Workaround: only ever drill into specific small fields (`pins.size`, `pins[0].history_id`, etc.), never request all locals on a frame whose `state` is in scope.
2. **dev profile strips locals** (`Cargo.toml: profile.dev.debug = "line-tables-only"`). Must use the `debugger` profile for variable inspection. Build takes ~5 min.
3. The default `cargo test` build is sufficient if you only need to confirm the *symptom* (passes 47/50, no edit transitions fire); only switch to `--profile debugger` when you need to see locals.

## Three fix options (ranked)

The user asked me to recommend, but didn't pick one. Pick before next session.

1. **Fix `region_predictable` to walk layout-block children of `root_id`** (matches the `gpui_pbt_edit_transitions_starvation.md` memory). Most invasive, but cleanly addresses the documented gap and unlocks all edit paths in any seed configuration.
   - Entry point: `crates/holon-integration-tests/src/pbt/reference_state.rs` — search for `fn region_predictable` and `fn active_layout_renders_region` (case 3 is the misclassification).
2. **Pre-seed Main with a real document at startup.** Have the PBT setup write an `index.org` (via the existing `WriteOrgFile` setup transition) that has Text children under a layout that the test pins into Main. Lowest-risk, but feels like papering over the real bug.
3. **Extend `CreateDocument` (or add an `OpenDoc` transition) to also pin the new doc into Main's `open_pins`.** Mid-risk, gives the PBT a way to navigate into editable content during the run instead of at startup. Run shows `CreateDocument` fires 3× already, so it's a natural extension point.
   - Generator: `crates/holon-integration-tests/src/pbt/transitions/create_document.rs`.

Once any of these lands, re-run the same command. If `SplitBlock`/`ClickBlock`/etc. start firing, the question "does this detect `split_block` bugs?" becomes meaningful — and at that point we'll see whether the `split_block` bugs the user mentioned actually surface.

## Open debugger session

I left `f231bb46-5a80-452c-9dc3-09003de3c587` connected at the time of this writeup (rules say not to disconnect without asking, and the user was wrapping up). On next session entry, either:
- Tell the new agent to disconnect via `mcp__debugger_mcp__debugger_disconnect`, or
- Start fresh — the prior process will eventually be cleaned up.

## Files / lines to know

- `crates/holon-integration-tests/src/pbt/reference_state.rs:1187` — `main_editable_descendants` (the filter that's returning empty)
- `crates/holon-integration-tests/src/pbt/reference_state.rs:1564` — `expected_focus_root_ids` (proven non-empty, with the layout block as pin)
- `crates/holon-integration-tests/src/pbt/reference_state.rs:1063` — `focusable_rendered_block_ids` (ClickBlock's path; also returns empty)
- `crates/holon-integration-tests/src/pbt/transitions/split_block.rs:42` — the call site `state.main_editable_descendants()` whose empty return starves SplitBlock
- `crates/holon-integration-tests/src/pbt/transitions/indent.rs:35` — same chain via `main_editable_descendants` + previous_sibling filter
- `crates/holon-integration-tests/src/pbt/live_geometry.rs:79` — `filter_to_rendered` (downstream filter; the ref-state output is already empty before this)
- `frontends/gpui/tests/gpui_ui_pbt.rs` — test entry point; PBT runs on bg thread, GPUI on main
- Memory: `~/.claude/projects/.../memory/gpui_pbt_edit_transitions_starvation.md` — long-standing record of this gap

## Run logs

- `/tmp/gpui_ui_pbt.log` — first run (nextest, failed enumerate)
- `/tmp/gpui_ui_pbt2.log` — full 50-step PBT run, `passed: 47/50`, the canonical "no edit transitions" log
- `/tmp/gpui_pbt_dbg_build.log` — debugger-profile build log
