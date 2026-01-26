# Design: PBT Coverage for Signal Loop (ReactiveEngine → GPUI → Operation)

## Problem

The cursor-jump-back bug lived in the gap between ReactiveEngine and GPUI:
- CDC re-emitted `current_editor_focus` matview rows on unrelated DB writes
- `Mutable::set()` always notifies (no dedup) → signal fired with stale data
- GPUI consumer overrode user-initiated focus, yanking cursor back

Existing PBTs test up to `ReactiveEngine.snapshot() → ViewModel` but don't exercise:
1. Signal emission + dedup (`watch_editor_cursor`)
2. Signal consumption (focus apply/skip logic)
3. Operation feedback loops (operation → CDC → signal → operation)

## Design

### Core Idea: `GpuiMutationDriver` implements `MutationDriver`

The existing `MutationDriver` trait dispatches operations. The GPUI driver goes through
the **full GPUI loop** including signal consumption, instead of just `dispatch_intent`.

```
MutationDriver::apply_ui_mutation("navigation", "editor_focus", params)
    ↓
GpuiMutationDriver:
    1. services.set_focus(block_id)          ← simulates click's set_focus
    2. services.dispatch_intent(editor_focus) ← simulates click's operation
    3. drain CDC + signal loop               ← wait for feedback to settle
    4. assert: signal didn't re-apply focus  ← the invariant
```

### `GpuiMutationDriver`

Lives in `frontends/gpui/` (depends on gpui types). Implements `MutationDriver`.

```rust
pub struct GpuiMutationDriver {
    engine: Arc<ReactiveEngine>,
    /// Tracks cursor signal emissions for invariant checking.
    cursor_emissions: Arc<Mutex<Vec<(String, i64)>>>,
    /// Handle to the cursor signal listener task.
    _cursor_listener: JoinHandle<()>,
}
```

**Construction**: Takes `ReactiveEngine`, subscribes to `watch_editor_cursor()`,
records all emissions into `cursor_emissions`.

**`apply_ui_mutation` for `editor_focus`**:
1. Call `engine.set_focus(Some(block_id))` — same as GPUI click handler
2. Call `engine.dispatch_intent(OperationIntent::new("navigation", "editor_focus", params))`
3. `tokio::time::sleep(50ms)` — let CDC propagate
4. Check `cursor_emissions`: the signal should have fired at most once with this block_id
5. Clear emissions for next operation

**`apply_ui_mutation` for everything else**: Delegate to `ReactiveEngineDriver`.

### New Transition: `SimulateMcpResync`

Exercises the exact bug pattern — an unrelated DB write that triggers IVM re-evaluation:

```rust
E2ETransition::SimulateMcpResync {
    /// Write some dummy data to an unrelated table to trigger IVM
    table: String,
}
```

In the SuT: `INSERT OR REPLACE INTO {table} (id, data) VALUES ('pbt_probe', 'x')`.
Then wait for CDC to settle. The invariant: cursor signal must NOT re-fire.

### New Invariant: `check_cursor_signal_stability`

After every `NavigateFocus` or `editor_focus` dispatch, and especially after
`SimulateMcpResync`:

```rust
fn check_cursor_signal_stability(driver: &GpuiMutationDriver) {
    let emissions = driver.cursor_emissions.lock().unwrap();
    // After settling, there should be 0 new emissions
    // (the set_if dedup should suppress re-emissions)
    assert_eq!(emissions.len(), 0, "cursor signal fired unexpectedly: {emissions:?}");
}
```

### Integration into Existing PBT

**No new PBT binary** — the existing `general_e2e_pbt.rs` gains an optional
`GpuiMutationDriver` when run with a GPUI window (via `gpui_ui_pbt.rs`).

**Profile idea** (future): A `PbtProfile` enum controls which transitions are generated:

```rust
enum PbtProfile {
    Full,                    // all transitions (default)
    FocusAndSignal,          // NavigateFocus + editor_focus + SimulateMcpResync only
    OrgSync,                 // WriteOrgFile + ApplyMutation only
    // ...
}
```

This lets us reproduce signal bugs fast without 100+ unrelated transitions.

### How It Composes with Existing SuT

The `E2ESut` already has:
- `install_driver()` that picks `ReactiveEngineDriver` or `DirectMutationDriver`
- `with_driver(driver)` constructor for Flutter

For GPUI: `E2ESut::with_driver(Box::new(GpuiMutationDriver::new(engine)))`.

The GPUI PBT binary (`gpui_ui_pbt.rs`) already creates a `FrontendSession` and
`ReactiveEngine` on the main thread. It just needs to pass the engine to the PBT
thread so it can construct `GpuiMutationDriver`.

### What This Catches

1. **CDC re-emission loops** (the cursor-jump-back bug): SimulateMcpResync after
   NavigateFocus triggers IVM re-evaluation; invariant detects if signal re-fires.

2. **set_focus / dispatch_intent ordering**: If click handler sets focus AFTER
   dispatching editor_focus (wrong order), the blur guard fails.

3. **Mutable::set vs set_if regression**: If someone changes `set_if` back to `set`,
   the invariant catches the repeated signal emissions immediately.

4. **pending_cursor render loop**: If pending_cursor re-buffering returns, the
   `GpuiMutationDriver` can track render counts and detect the loop.

### What This Does NOT Cover (and Why)

- **Visual layout correctness**: Use `GeometryDriver` + xcap screenshots for that.
- **Keyboard input simulation**: Use `send_key_chord()` (already tested).
- **GPUI-specific rendering bugs**: Those need the real GPUI window. This design
  tests the signal/focus/operation loop which is frontend-agnostic.

### Implementation Sequence

1. **Extract cursor signal consumption logic** from `lib.rs` into a testable
   function: `fn apply_cursor_focus(block_id, offset, currently_focused) -> FocusAction`
2. **Create `GpuiMutationDriver`** in `frontends/gpui/src/` (or a shared test crate)
3. **Add `SimulateMcpResync` transition** + generator
4. **Add `check_cursor_signal_stability` invariant** to existing invariant set
5. **Wire into `gpui_ui_pbt.rs`** — pass engine to PBT thread, construct driver
6. **(Future) PbtProfile** for focused transition generation
