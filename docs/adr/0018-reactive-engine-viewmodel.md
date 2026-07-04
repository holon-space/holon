# ADR 0018: Replace the CDC event-bus / AppState / BlockWatchRegistry model with a `ReactiveEngine` driving an observable ViewModel tree

Status: Accepted (retroactive — documenting shipped architecture)
Date: 2026-07-06

> Written after the fact to record a decision the code already embodies. The
> old frontend reactive stack (a CDC accumulator feeding an `AppState`, plus a
> `BlockWatchRegistry` fanning change events out to views over channels) has
> been removed and replaced by a single `futures-signals`-based
> `ReactiveEngine` (`crates/holon-frontend/src/reactive.rs`) that owns a
> persistent, observable `ReactiveViewModel` tree which every frontend
> (GPUI, TUI, and the headless PBT harness) subscribes to as a `Signal`. The
> replaced module is now a tombstone (`crates/holon-frontend/src/cdc.rs`).

## Context

The previous data-flow was three cooperating pieces:

1. A **`CdcAccumulator`** that folded Turso IVM change events into a row map
   (still present as a pure reducer in `holon-api`,
   `crates/holon-api/src/reactive.rs:343`, but no longer the frontend spine).
2. An **`AppState`** the accumulator wrote into, plus a `spawn_ui_listener` /
   `CdcState` pump.
3. A **`BlockWatchRegistry`** that views registered with to receive change
   notifications, typically over channels.

The shape had three structural problems:

- **The cache, the change-fold, the join-to-render, and the API to the view
  were four separate objects** wired together by hand, with the event bus in the
  middle. Every new reactive surface meant threading another channel and another
  registration through all four.
- **Views were push-notified, then re-pulled.** A change event told a view
  "something changed"; the view then re-read `AppState`. Coalescing, ordering,
  and "did my specific block change?" all had to be re-derived downstream.
- **Rebuilds threw away node identity.** A focus change or a data-only edit that
  triggered a re-render replaced whole subtrees, which — for an editing surface
  — destroys editor/caret/scroll state and (for GPUI) spawns duplicate cursors.

We wanted a model where *the cache is the signal source*, views *observe* rather
than get-notified-then-re-pull, and per-node UI state (expand, focus, caret,
view mode) survives data updates.

## Decision

Adopt `futures-signals` and collapse the four objects into one engine plus an
observable node tree. The module header states the intent directly
(`crates/holon-frontend/src/reactive.rs:1-10`):

> Replaces CdcAccumulator + BlockWatchRegistry + AppState with a single
> reactive cache. Each watched block or live query gets a `ReactiveRenderedRows`
> that IS the cache, the accumulator, AND the signal source.
> `Turso IVM → UiEvent → ReactiveRenderedRows → Signal<ViewModel> → Stream → Frontend`

### 1. `ReactiveRenderedRows` — one object that is cache, fold, and signal

`ReactiveRenderedRows` (`reactive.rs:621-633`) holds a `Mutable<RenderExpr>` (how
to render) composed with a `ReactiveRowSet` (the data), plus a `data_generation`
atomic. `apply_event` (`reactive.rs:681-...`) is the **single entry point for all
CDC events**: a `UiEvent::Structure` sets the render expression and bumps the
generation; a `UiEvent::Data` batch diffs rows into the row set, discarding
stale generations. There is no separate accumulator and no separate notify step —
mutating the `Mutable`s *is* the notification, because downstream signals are
derived from them.

Generation tracking solves the ghost-row problem: the first data batch of a new
generation is the authoritative snapshot, applied then key-retained so rows the
new query no longer returns are dropped — while surviving rows **keep their
`Mutable` cell identity** and the set is never momentarily empty
(`reactive.rs:625-632, 698-707`).

### 2. `ReactiveViewModel` — a persistent, observable node tree (the one-writer rule)

Interpreting a `RenderExpr` against rows produces a `ReactiveViewModel`
(`crates/holon-frontend/src/reactive_view_model.rs:304-367`) — the node the
frontend actually renders. The persistent-node shape is the crux of the rewrite:

- A node's **data is a `ReadOnlyMutable<Arc<DataRow>>`** (`reactive_view_model.rs`
  `data` field). The *only* writable handle to a row's cell lives inside
  `ReactiveRowSet.data`, and `apply_change` is its sole writer. Every downstream
  node — every leaf widget — holds a `ReadOnlyMutable` clone, so a leaf calling
  `.set()` on row data is a **compile error**. This is the type system enforcing
  a one-writer rule, so a data update refreshes existing cells in place instead
  of rebuilding the subtree.
- **UI-local state is separate and freely mutable**: `expanded: Option<Mutable<bool>>`,
  `props: Mutable<HashMap<String, Value>>`, plus captured `render_ctx` and an
  `interpret_fn` closure so a node can recompute its own props from new data
  without re-running the full interpret pipeline (`reactive_view_model.rs:304-367`).
- **Reactive collections** hang off `collection: Option<Arc<ReactiveView>>`.
  `ReactiveView` (`crates/holon-frontend/src/reactive_view.rs:78-89`) owns a
  `MutableVec<Arc<ReactiveViewModel>>` the frontend subscribes to via
  `children_signal_vec()`; its driver pushes new children into the vec, so
  incremental list changes never rebuild the parent.

The consequence is that "re-render" becomes "update the changed `Mutable`s", and
node identity (hence editor/caret/scroll state) is preserved across data edits.

### 3. `ReactiveEngine` — the owner and the `BuilderServices` implementer

`ReactiveEngine` (`reactive.rs:1187-1214`) owns the watcher table
(`watchers: Mutex<HashMap<EntityUri, WatcherState>>`), the shared shadow
interpreter, the row-provider cache, the keybinding registry, the `UiState`, and
the session/runtime handles. Frontends never see it directly — they interpret
through the narrow `BuilderServices` trait (`reactive.rs:54-...`), of which
`ReactiveEngine` is the real implementation. `watch_signal` /
`watch_data_signal` (`reactive.rs:1294-...`) hand a frontend a
`Pin<Box<dyn Signal<Item = ReactiveViewModel>>>` it polls directly from a GPUI
`cx.spawn` — CDC writes from the tokio side wake the signal cross-thread, with no
intermediate channel. The same engine backs the headless PBT harness (which runs
windowless but drives the *real* engine, so test fidelity is intact —
`reactive.rs:43-52`), which is why the whole surface is expressed as observable
signals rather than GPUI-specific callbacks.

### 4. `UiState` — window-global singletons for pure UI state (focus)

Some state is neither per-row data nor per-node: what block is focused, the
viewport, the pending caret seed. These live in one `UiState`
(`reactive.rs:951-981`) owned by the engine — a set of window-global `Mutable`
singletons. Focus is the load-bearing one: `focused_block: Mutable<Option<EntityUri>>`
(`reactive.rs:953`) is the single source of truth for `is_focused` in predicate
evaluation. Two deliberate design choices are recorded in-code:

- **Focus does not bump the viewport/ui generation** (`set_focus`,
  `reactive.rs:1014-1042`). Focus is pure UI state GPUI handles via
  `window.focus()`; bumping a generation would make live-query shells replace
  their entire tree — re-creating editors for every row and spawning multiple
  cursors. So focus mutates its own signal and *only* that.
- **`set_focus` is crate-private** — external callers must go through the
  `navigation.focus` intent so `maybe_mirror_navigation_focus`
  (`reactive.rs:2464-...`) keeps the SQL nav-history table in sync. The
  visibility narrowing was an explicit door-closing so test/frontend code can't
  reopen the direct-mutation path.
- A `pending_caret_seed` `Mutable` carries the initial caret offset to the next
  editor that mounts for a block (`reactive.rs:965-972`), replacing the old
  `editor_cursor` round-trip — the in-process way a focus move positions a caret.

`focused_occurrence` (`reactive.rs:973-980`) is an *additive spike* for
display-placement (relates to ADR 0010 / ADR 0016 occurrence-keyed focus): it
proves the focus authority can carry `(id, occurrence)` without widening
`focused_block`'s type across its ~10 readers and all four frontends.

## Consequences

- **One reactive spine.** A watched block or live query is one
  `ReactiveRenderedRows` that is simultaneously the cache, the CDC fold, and the
  `Signal` source. New reactive surfaces subscribe to a signal instead of
  registering with a bus and re-pulling `AppState`. `cdc.rs` is now a four-line
  tombstone pointing at the replacement (`crates/holon-frontend/src/cdc.rs:1-4`;
  `crates/holon-frontend/src/lib.rs:102`).
- **Node identity survives updates.** The `ReadOnlyMutable<Arc<DataRow>>`
  one-writer rule + generation-retention mean data edits refresh cells in place;
  editor, caret, and scroll state persist, and GPUI does not get duplicate
  cursors on focus change.
- **The type system enforces the write discipline.** A leaf widget *cannot*
  mutate row data — it is a compile error, not a convention. This is the strong,
  code-level signal the next agent reads.
- **Disclosed debt: `ReactiveEngine` is a god-class.** This is admitted in-tree
  with `// TODO: This looks like a god-class heavily violating SRP`
  (`reactive.rs:1186`). It owns watchers, the interpreter, the provider cache,
  keybindings, `UiState`, dispatch, and the entire `BuilderServices` surface
  (`reactive.rs` is ~3,150 lines, ~186 methods). The `BuilderServices` trait is
  itself very wide (focus, drawers, query watching, operation dispatch, viewport,
  keybindings, profiles) — a natural next split, not yet done.
- **Window-global focus is a singleton, with a reserved graduation.**
  `focused_block` being a single `Mutable<Option<EntityUri>>` is simple and
  correct for one focused block per window, but display-placement (the same
  block shown in multiple positions) needs `(id, occurrence)`. Rather than widen
  the type now, `focused_occurrence` was added as an additive spike; graduating
  it is deferred (see ADR 0010, ADR 0016).
- **`futures-signals` is now a load-bearing dependency.** Cross-thread wakeups
  (tokio CDC → GPUI poll) rely on its signal semantics; the mental model
  ("mutate a `Mutable`, subscribers recompute") must be understood to work in
  this crate. `map_ref`/`SignalVec` composition replaces the old channel plumbing.

## Alternatives considered

- **Keep the event bus, add coalescing.** Rejected: it leaves four objects and
  the notify-then-re-pull pattern intact; the identity-loss-on-rebuild problem is
  untouched because views still re-read a shared `AppState`.
- **A single mutable app-state tree the views diff.** Rejected: diffing to
  recover "what changed" is exactly what generation-tracked `Mutable` cells give
  for free, and a hand-rolled diff can't guarantee cell-identity preservation the
  way the `ReadOnlyMutable` one-writer rule does.
- **GPUI-native reactivity (entities/contexts) as the spine.** Rejected: the
  same engine must drive the TUI and the windowless PBT harness. Expressing the
  reactive layer as framework-agnostic `Signal`s keeps one engine behind all
  frontends; GPUI just polls the signal.
- **Split `ReactiveEngine` before shipping.** Deferred, not rejected — the
  god-class is disclosed debt. Consolidating the reactive model first (one spine
  that works across frontends) was prioritised over premature decomposition.
