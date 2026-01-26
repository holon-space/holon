# Reactive ViewModel PoC — Session Handoff

## What we're doing

Building a proof-of-concept of the **target UI architecture** (see `ARCHITECTURE_UI.md`) as a GPUI demo + test suite, validating each aspect before touching the production codebase. The PoC lives in:

- **Shared types**: `frontends/gpui/src/reactive_vm_poc.rs` — `ReactiveNode`, `ItemNode`, `Interpreter` trait, mini interpreter, sample data, RenderExpr helpers, `TreeSnapshot`
- **Interactive demo**: `frontends/gpui/examples/reactive_vm_demo.rs` — 3-panel launchable app
- **Tests**: `frontends/gpui/tests/reactive_vm_test.rs` — headless (harness=false, uses `gpui::run_test_once`)
- **Real-window test**: `frontends/gpui/tests/reactive_vm_realwindow_test.rs` — uses `Application::run()` on main thread (real macOS event loop, not TestAppContext)

Run demo: `cargo run --example reactive_vm_demo -p holon-gpui`
Run tests: `cargo test -p holon-gpui --test reactive_vm_test`
Run real-window test: `cargo test -p holon-gpui --test reactive_vm_realwindow_test`

## What's validated (39 headless + 1 real-window tests, all pass)

1. `Mutable<bool>` signal → GPUI `cx.notify()` bridge works
2. `Mutable::clone()` shares signal source (subscriptions survive); `Mutable::new(old.get())` does not
3. Shared `Mutable<RenderExpr>` for item template — setting it once propagates to all `ItemNode`s via `map_ref!`
4. Per-item `Mutable<Arc<DataRow>>` — data push to one item doesn't affect others
5. Template change then data change — `map_ref!` correctly uses the new template
6. `ReactiveNode` structural changes — `apply_expr` keeps/adds/removes children correctly
7. Children share parent's `Mutable<Arc<DataRow>>` (synchronous `.get()` confirms shared signal source)
8. GPUI rendered output updates after data change (window-level test)
9. Child `render()` is called after data-only change (render counter test, no parent read)
10. **`Interpreter` trait boundary** — custom interpreter (`PrefixInterpreter`) proves trait-object dispatch works; nodes use `interpreter.interpret()` instead of free function
11. **`TreeSnapshot`** — synchronous recursive tree read works without `run_until_parked()`; MCP/PBT can consume ViewModel state directly
12. **Nested collections (3 levels)** — `row(bold(text(col)), row(badge(col), text(col)))` data change at root propagates to all 3 leaf nodes
13. **Structural change at nested level** — expanding `row(text)` to `row(row(text, badge))` works; subsequent data changes reach the new nested children
14. **Entity drop → signal task cleanup** — signal tasks exit when the entity is freed (via `WeakEntity::update()` returning `Err` → break). Verified for both direct entity drops and collection child removal.
15. **VecDiff-driven `ReactiveCollection`** — `MutableVec<Entity<ReactiveNode>>`-backed collection supports InsertAt (new entity), RemoveAt (drops entity + cleans signal), UpdateAt (data.set on existing node — no rebuild, entity ID preserved), Move (reorder without rebuild). Full insert→update→move→remove sequence validated.

## Panel C rendering bug — RESOLVED

**Original symptom**: Clicking "Change data" in Panel C did nothing visible.

**Root cause**: Frame timing — `PanelC::change_data()` set the data Mutable but didn't call `cx.notify()` on the root entity. PanelC's own `cx.notify()` triggered a re-render before the signal fired, showing stale cached children. The signal then fired asynchronously but the user perceived a one-frame delay.

**Fix (two parts)**:
1. `ReactiveNode::render()` now computes display fresh from `data.get_cloned()` + `expr.get_cloned()` via `interpreter.interpret()` instead of reading the `display` Mutable (which lags by one async tick). The signal still updates `display` for non-GPUI consumers.
2. `PanelC::change_data()` now calls `cx.notify()` on the root entity so GPUI re-renders the root's subtree immediately.

**Note**: The bug could not be reproduced in tests (`run_until_parked()` drains all async work including signals). The render counter test confirms the signal→notify→render path works correctly in the test environment.

## Architecture patterns validated

| Pattern | Status | Test(s) |
|---------|--------|---------|
| Mutable signal → GPUI bridge | ✅ | #1-3 |
| Shared Mutable broadcast | ✅ | #3, #7 |
| Per-node self-interpretation | ✅ | #4-5 |
| Structural push-down (apply_expr) | ✅ | #6, #8-9 |
| Interpreter trait boundary | ✅ | #10 |
| Synchronous tree snapshot (MCP/PBT) | ✅ | #11 |
| Multi-level signal propagation | ✅ | #12-13 |
| Live render (no async lag) | ✅ | via render() always computing fresh |
| Expand toggle (lazy children) | ✅ | #18-21 |
| Per-item data Mutables (collections) | ✅ | #22-24 |
| Entity drop → signal cleanup | ✅ | #25-26 |
| VecDiff-driven insert/remove/update/move | ✅ | #27-33 |
| Named arg ordering in apply_expr | ✅ | #34-35 |
| Expand-triggers-interpretation | ✅ | #36-37 |
| Concurrent data + template mutation | ✅ | #38-39 |

## Convergence gap: PoC vs production

| ARCHITECTURE_UI.md principle | PoC | Production |
|---|---|---|
| Persistent nodes, built once | `ReactiveNode` — built once, updated via Mutables | `ReactiveViewModel` — rebuilt from scratch every signal fire |
| Per-node self-interpretation via `map_ref!` | Each node owns `Mutable<RenderExpr>` + `Mutable<Arc<DataRow>>` | Interpretation is a one-shot call from the driver |
| Push-down updates | `apply_expr` pushes to children recursively | No push-down — new tree replaces old |
| Shared Mutable broadcast | Clone of template Mutable to all items | Template is a static `RenderExpr`, re-interpreted per row |
| State on the node | `expanded: Option<Mutable<bool>>` on ReactiveNode | Engine-level `expand_state_cache` HashMap, re-fetched by key |

Production state survival works via a workaround: engine-level HashMap caches (`expand_state_cache`, `view_mode_cache`) for Mutables, re-fetched by entity key on each rebuild. The target architecture moves this state onto the persistent nodes themselves.

Key production files:
- `reactive.rs:601-620` — structural_signal_with_ui_gen: full tree rebuild on render_expr change
- `reactive.rs:1003-1057` — watch_live: spawns per-block watcher
- `reactive.rs:1596-1612` — get_or_create_expand_state / get_or_create_view_mode: persistent caches
- `reactive_view.rs:567-730` — flat collection driver: per-row re-interpret on VecDiff
- `reactive_view.rs:43-66` — row_render_context: builds per-row RenderContext with profile ops

## Approach: de-risk via demo, converge on target architecture

The demo is NOT an evolution of the current codebase — it's a clean-slate implementation of the target architecture. It should converge on `ARCHITECTURE_UI.md`, not on something halfway between current and target.

Rules:
- **Reproduce bugs in tests before fixing** — if a behavior works in the demo but fails in tests (or vice versa), understand why before changing code
- **No fallbacks to current architecture patterns** — no centralized HashMaps, no `ui_generation`, no ephemeral tree rebuilds, no reconciliation tree walks
- **Shared code** — `reactive_vm_poc.rs` is the single source of truth; demo and tests import from it
- **Every new pattern gets a test** — if the demo validates something, the test suite must cover it too

## What to build next (in priority order)

### Phase 1 — DONE (tests 18–24 all pass)

1. ~~**Expand toggle with lazy loading**~~ — `new_expandable()` with `Mutable<bool>`, lazy child creation/destruction, data propagation after expand. Tests: 18–21.

2. ~~**Per-item data Mutables in collections**~~ — `new_collection()` distributes independent `Mutable<Arc<DataRow>>` per child. Template broadcast via shared `Mutable<RenderExpr>`. Tests: 22–24.

### Phase 2 — De-risk before production refactor

Five gaps between the PoC and production that can still be validated cheaply in the demo before touching `reactive.rs` / `reactive_view.rs`.

**2a. Signal task leak on entity drop — DONE (tests #25-26)**

Signal tasks used `.detach()` and leaked when the entity was freed. Investigation revealed:
- `cx.spawn()` passes a `WeakEntity<T>` (not strong), so the task doesn't keep the entity alive
- GPUI defers entity cleanup to `flush_effects()`, NOT `run_until_parked()` — tests must trigger flush via `cx.new()` or `cx.update()` after drop
- Fix: signal tasks check `this.update()` result and `break` on `Err` (entity gone). Also stored `Task` handles on the struct (`_tasks: Vec<Task<()>>`) — cancelled when entity is freed
- Same pattern applied to `ItemNode._signal_task` and `ReactiveNode._tasks` (signal + expand tasks)

**2b. VecDiff-driven collection mutations — DONE (tests #27-33)**

Added `ReactiveCollection` backed by `MutableVec<Entity<ReactiveNode>>`. Validated:
- `InsertAt` → creates new entity, spliced into position
- `RemoveAt` → drops entity, signal task cleaned up (verified separately in #32)
- `UpdateAt` → `data.set(new_row)` on existing node — **entity ID preserved** (no rebuild)
- `Move` → rearranges children without re-creating nodes — **all entity IDs preserved**
- Full sequence test: insert→update→move→remove in succession

**2c. Named arg ordering in `apply_expr` (MEDIUM)**

`apply_expr` matches children by positional index. Production uses named args (`expand_toggle(#{header: ..., content: ...})`). Rhai `#{}` maps have no guaranteed iteration order — two passes could produce args in different order, causing `apply_expr` to match wrong children.

**2c. Named arg ordering — DONE (tests #34-35)**

Fix: `apply_expr` now detects when all args are named and matches by `arg.name` instead of positional index. Uses a `HashMap<&str, usize>` to look up the old child by name, reuses the entity if the kind matches, creates new entities for new names. Swapping `#{a: text(...), b: badge(...)}` to `#{b: badge(...), a: text(...)}` preserves both entity IDs.

**2d. Expand-triggers-interpretation — DONE (tests #36-37)**

Expand already calls `build_children()` which creates child ReactiveNodes with signal tasks. A `CountingInterpreter` verified that expand triggers new interpretation calls. Data changes after expand propagate through the new children's signal tasks. The PoC's structural decomposition IS interpretation for its simple expressions — the gap with production (where `interpret(content_template)` spawns live queries) is a type-system issue (String vs ReactiveViewModel), not a signal-mechanics issue.

**2e. Concurrent data + template mutation — DONE (tests #38-39)**

Confirmed `map_ref!` coalesces concurrent `data.set()` + `apply_expr()` correctly. Also tested 10 rapid data mutations without draining — final snapshot and display Mutable both reflect `v10`. No intermediate garbage.

### Phase 3 — Production convergence

3. **Connect to production RenderInterpreter** — swap `DefaultInterpreter` for an implementation that delegates to `holon-frontend`'s `RenderInterpreter`. This validates the real interpretation pipeline works through the trait boundary.

4. **MCP tool for snapshot** — expose `tree_snapshot()` as an MCP tool so the live ViewModel can be inspected from Claude Code during development.

## Files

| File | Purpose |
|------|---------|
| `ARCHITECTURE_UI.md` | Target architecture document |
| `frontends/gpui/src/reactive_vm_poc.rs` | Shared reactive ViewModel types (ReactiveNode, ItemNode, Interpreter, TreeSnapshot) |
| `frontends/gpui/examples/reactive_vm_demo.rs` | Interactive 3-panel demo |
| `frontends/gpui/tests/reactive_vm_test.rs` | Headless test suite (39 tests, all passing) |
| `frontends/gpui/tests/reactive_vm_realwindow_test.rs` | Real-window test — uses `Application::run()`, verifies signal→render in real event loop |
| `frontends/gpui/Cargo.toml` | Has `[[example]]` and `[[test]]` entries for all |
