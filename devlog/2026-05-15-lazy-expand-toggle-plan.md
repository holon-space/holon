# Lazy materialization for gated widgets — design + plan (revised: E7)

## Goal
When a "gated" widget's gate flips false→true (e.g. expand_toggle's
`expanded`, tabs' `active_tab`, view_mode_switcher's `current_mode`),
materialize the inner content_template against the captured
RenderContext and surface it as the widget's children. While the gate
is false, do no work — no FDW fetches, no `live_query` subscriptions.
Once materialized, children are fully reactive and the cache persists
across gate toggles for the lifetime of the VM.

Motivating use cases:
- GitHub.org `Repositories with Activity` — expand one repo to see its
  issues; don't fetch every repo's issues just because the page loaded.
- Tabs — switch to GitHub tab; other tabs' live_queries shouldn't run.
- view_mode_switcher — switching tree↔table shouldn't leave the
  inactive mode's subscriptions live.

## Decision: option E7 (new `LazyReactiveSlot` type)

Reviewed C (per-widget bespoke laziness), D (lazy ReactiveSlot
primitive across all consumers), and E1–E6 alternatives. Settled on E7:

> Introduce a new `LazyReactiveSlot` type, sibling to `ReactiveSlot`.
> ViewModel grows `lazy_slot: Option<LazyReactiveSlot>` alongside
> `slot`. Widgets that want lazy semantics populate `lazy_slot`;
> existing eager slots stay on `slot` unchanged.

### Why E7 over C
- ≥4 widget consumers on the horizon (expand_toggle, tabs,
  view_mode_switcher, conditional render); per-widget bespoke
  laziness duplicates the push_down landmine 4 times.

### Why E7 over D
- D's bound propagation (`Send + Sync + 'static` on the thunk) would
  infect every existing `ReactiveSlot` consumer (live_block,
  live_query, view_mode_switcher). E7 confines those bounds to the
  new type — opt-in for lazy widgets.
- D's silent-failure surface (un-forced slot → empty) hits every
  consumer. E7 contains it to `LazyReactiveSlot`, where the failure
  mode is by construction, not accidental.
- D refactors every slot site; E7 is additive (new type + new field
  + per-widget wiring).

### Trade-off accepted
Two slot types is a real mental tax — authors have to ask
"always-present or gated?" at construction time. We claim that
question is *meaningful*, not a leaky abstraction.

## Constraints discovered during design

1. **`ba.interpret` is `&'a dyn Fn`** — cannot capture into a `'static`
   thunk directly. Capture `Arc<dyn BuilderServices>` and call
   `services.interpret(expr, ctx)`. Needs a `clone_arc` accessor on
   the trait.

2. **`RenderContext` does not carry `services`** (the reactive.rs:40
   comment is stale). Services must be captured separately.

3. **`push_down_slot` overwrites old content with fresh** — for any
   lazy slot, fresh is always "empty / unmaterialized" because the
   builder runs with a fresh gate Mutable. We need
   `push_down_lazy_slot` to keep the old cache and adopt the new
   thunk+gate. `push_down_slot` is left alone — eager slots have
   different invariants.

4. **`expanded` Mutable already survives rebuilds** via `with_update`
   / `push_down_children`. `lazy_slot` must be wired the same way.

5. **Snapshot mutating the cache is bounded**: the cache flips
   `Option::None → Some(...)` exactly once per VM lifetime. After
   that, materialize_if_gated short-circuits.

## LazyReactiveSlot shape

```rust
pub struct LazyReactiveSlot {
    /// Captured deferred interpretation. Fires at most once per cache miss.
    pub thunk: Arc<dyn Fn() -> ReactiveViewModel + Send + Sync>,
    /// Cached materialized content. None until first gate==true read.
    pub cache: Mutable<Option<Arc<ReactiveViewModel>>>,
    /// External gate — typically the widget's own state Mutable
    /// (e.g. expand_toggle.expanded, tabs.active == this_tab).
    pub gate: ReadOnlyMutable<bool>,
}

impl LazyReactiveSlot {
    pub fn materialize_if_gated(&self) -> Option<Arc<ReactiveViewModel>> {
        if !self.gate.get() { return None; }
        if let Some(v) = self.cache.get_cloned() { return Some(v); }
        let vm = Arc::new((self.thunk)());
        self.cache.set(Some(vm.clone()));
        Some(vm)
    }
}
```

## Implementation order

1. **#1 Plan devlog** (this file). ✅
2. **#2 `BuilderServices::clone_arc`** — trait + ReactiveEngine impl;
   other impls default-panic.
3. **#3 Define `LazyReactiveSlot`** — type, methods, push_down helper.
4. **#4 Add `lazy_slot` field** to ReactiveViewModel — preserve across
   with_update / push_down_children.
5. **#5 Port `expand_toggle`** — first consumer. Captures thunk,
   sets lazy_slot, removes the old empty-slot dance.
6. **#6 Update snapshot path** — expand_toggle arm reads lazy_slot
   via materialize_if_gated().
7. **#7 Port tabs** (second consumer, validation). If the tabs widget
   doesn't exist yet, document the expected shape.
8. **#8 Unit test** — counting thunk, verify cache + idempotency +
   survival across gate toggles.

## Recipe for porting a widget to `lazy_slot`

After landing `expand_toggle`, the pattern for additional consumers is:

1. **Identify the gate.** A `Mutable<bool>` that reflects "is this slot's
   content currently demanded?":
   - `expand_toggle`: the `expanded` Mutable.
   - `view_mode_switcher` (per-mode lazy): one gate per mode,
     `Mutable::new(active_mode == this_mode)`, plus a subscription that
     flips them on switch.
   - `tabs`: one gate per tab, mirroring `active_tab == this_tab`.
   - `drawer`: the open/closed Mutable.

2. **Capture in the builder.**
   ```rust
   let services_arc = ba.services.clone_arc();
   let ctx = ba.ctx.clone();
   let template = ba.args.get_template("content").cloned()?;
   let thunk: Arc<dyn Fn() -> ViewModel + Send + Sync> =
       Arc::new(move || services_arc.interpret(&template, &ctx));
   let lazy_slot = LazyReactiveSlot::new(gate.read_only(), thunk);
   ```

3. **Wire into the VM.** Set `lazy_slot: Some(lazy_slot)` instead of
   `slot: Some(...)`. Keep the gate Mutable on the VM (e.g. `expanded`)
   so the snapshot path can read its state for the surrounding render.

4. **Update the snapshot arm.** Replace `self.slot.snapshot()` with:
   ```rust
   let content = self.lazy_slot
       .as_ref()
       .and_then(|s| s.materialize_if_gated());
   ```
   `None` → render the header/tab-bar only; `Some(vm)` → snapshot the
   materialised content normally.

5. **Don't carry the template through props.** The thunk owns it. This
   reverses the previous pattern of serialising `content_template` as
   a JSON prop for downstream reconstruction.

### Why `view_mode_switcher` is the natural second consumer

It already has a `Option<ReactiveSlot>` on the VM, and the snapshot arm
already reads through it. Today every mode's content is eagerly
interpreted at build time (eager `slot`); subscriptions in the
inactive mode's `live_query` stay live in the background. Moving to
`lazy_slot` per mode means switching to tree pauses table's
subscriptions until you switch back. The work is per-mode, so the
builder has to construct one `LazyReactiveSlot` per declared mode and
the snapshot arm needs to pick the active one.

Deferring this port keeps the current PR scoped to a single consumer
+ unit-test validation. Once #8 lands, `view_mode_switcher` is the
obvious next move.

## Out of scope (this PR)
- Suspend-on-collapse (pause inner subscriptions when gate flips
  false). `LazyReactiveSlot` is designed to admit this later without
  schema changes — add a `suspended: Mutable<bool>` plus a callback
  that pauses cached subscriptions.
- Cache eviction. Cache lives for VM lifetime. Revisit if widget
  counts grow into thousands.
- Migrating `view_mode_switcher` from `slot` to `lazy_slot`. Worth
  doing once expand_toggle + tabs validate the design.
- DSL-level lazy primitive (`when(...)`, `lazy(...)`). Could revisit
  if a generic conditional-render need emerges.
