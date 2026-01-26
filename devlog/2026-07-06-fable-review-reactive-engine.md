# Deep review: `ReactiveEngine` god-class (crates/holon-frontend/src/reactive.rs)

Reviewer: Fable agent, 2026-07-06. Read-only architecture review of the read/render
pipeline centerpiece. File is 3,145 lines, 14 types, 186 methods; the god-class
self-label is at `reactive.rs:1186` ("TODO: This looks like a god-class heavily
violating SRP").

---

## 1. Map of the engine

### Types in the file (all `crates/holon-frontend/src/reactive.rs`)

| Type | Lines | Role |
|---|---|---|
| `trait BuilderServices` | 54–404 | ~30-method capability trait builders see (`ctx.services`) |
| `ReactiveRowSet` | 430–586 | CDC diff accumulator: `MutableBTreeMap<EntityUri, Mutable<Arc<DataRow>>>` + generation token |
| `ReactiveRenderedRows` | 621–859 | one query's `Mutable<RenderExpr>` + `ReactiveRowSet` + signal combinators |
| `ReactiveRegistry` | 890–913 | `Mutex<HashMap<EntityUri, Arc<ReactiveRenderedRows>>>` |
| `WatcherState` | 917–923 | tokio task + command channel + manual `refcount: usize` |
| `ViewportInfo` / `UiState` | 944–1177 | window-global focus/viewport/caret singletons (focused_block at :953) |
| `ReactiveEngine` | 1187–1875 | everything else (see responsibilities below) |
| `impl BuilderServices for ReactiveEngine` | 1931–2277 | the facade wiring |
| `StubBuilderServices` | 2287–2420 | gallery/test stub with process-global tokio runtime |
| `BuilderServicesSlot` / `RenderInterpreterFn` / DI ext | 2429–2462 | OnceLock circularity plumbing |
| free fns (focus mirroring, dispatch chain, interpret_pure, make_interpret_fn) | 2464–2638 | |
| `LiveBlock` | 2648–2653 | tree + structural_changes stream |

### Signal graph (futures-signals)

Sources:
- `ReactiveRowSet.data: MutableBTreeMap<EntityUri, Mutable<Arc<DataRow>>>` — outer map diffs =
  row add/remove; inner `Mutable` per row = field updates (`set_neq`, so CDC echoes dedup). Sole
  writer: `apply_change` (:458) — single-writer by convention only (doc admits it at :422–425).
- `ReactiveRenderedRows.render_expr: Mutable<RenderExpr>` (:622) — structure.
- `UiState.viewport_generation: Mutable<u64>` (:959), `focused_block: Mutable<Option<EntityUri>>`
  (:953), `viewport: Mutable<Option<ViewportInfo>>` (:964), `pending_caret_seed` (:972),
  `focused_occurrence` spike (:980).
- `key_bindings: MutableBTreeMap<String, KeyChord>` (:1199).

Combinators on `ReactiveRenderedRows`:
- `reactive_signal[_with_ui_gen]` (:768/:782): `map_ref!(expr, data_signal, ui_gen)` →
  **full re-interpretation of the whole block tree on every emission**, explicitly documented
  at :766. `data_signal` (:570) flattens *every* per-row cell into "full Vec on any change".
- `structural_signal[_with_ui_gen]` (:812/:835): fires on expr / ui-gen only; per-row updates
  flow instead through `ReactiveView` drivers (`reactive_view.rs`) subscribed to
  `keyed_signal_vec()` / `row_mutable()` → in-place `MutableVec` patching.

### CDC → ViewModel → View dataflow

```
Turso CDC → backend ui_watcher (merge_triggers, enrich per row)
  → session.watch_ui(block) stream of UiEvent{Structure|Data}
  → ReactiveEngine::ensure_watching (:1521) spawns 1 tokio task per block
  → ReactiveRenderedRows.apply_event (:681): Structure sets gen+expr;
    Data batches → ReactiveRowSet.apply_change per row (+ first-batch retain_keys :730)
  → signal graph → interpret_fn (make_interpret_fn :2628 → interpret_pure :2607
    → RenderInterpreter via BuilderServices::interpret) → ReactiveViewModel
  → Views:
    • GPUI root: frontends/gpui/src/lib.rs:1030 `engine.watch_signal(root)` → for_each →
      rebuild root_vm + reconcile + cx.notify()  [FULL reinterpret path]
    • GPUI live blocks: render/builders/live_block.rs:72 `services.watch_live(uri, ...)` →
      LiveBlock{tree, structural_changes}; ReactiveShell (views/reactive_shell.rs:129)
      renders tree, listens to structural_changes, per-row updates flow through
      ReactiveView MutableVecs; Drop → unwatch (reactive_shell.rs:855)
    • TUI: snapshot pipeline — mirrors `expr`/`props`/`data` mutables per node
      (frontends/tui/src/render/mod.rs:6), inline-edit resolves via snapshot_reactive
    • web/wasm + PBT/MCP: `watch_snapshot_stream` (:1341) / `snapshot` (:1441) —
      full-tree reinterpretation per event, focus included
```

---

## 2. God-class critique

`ReactiveEngine` (:1187–1214, 11 fields) currently owns **seven distinct responsibilities**:

1. **Watcher lifecycle / subscription registry** — `registry`, `watchers`, `ensure_watching`
   (:1521), `ensure_query_watching` (:1786), `unwatch` (:1858), `set_variant` (:1844).
2. **Signal-graph assembly** — `watch_signal` / `watch_data_signal` / `watch` /
   `watch_snapshot_stream` / `watch_live` / `watch_query_live` (:1294–1781).
3. **Snapshot resolution with cycle detection** — `snapshot` (:1441–1510) with thread-local
   VISITED/STACK + Drop guard; `snapshot_reactive` (:1513).
4. **UI state authority** — owns `UiState`; focus mirroring of navigation intents
   (`maybe_mirror_navigation_focus` :2471, `maybe_clear_focus_on_delete` :2514,
   `apply_structural_focus` :2558).
5. **Operation dispatch** — `dispatch_intent` (:2010), `dispatch_intent_sync` (:2087),
   plus free `dispatch_intent_chain` (:2583). Three near-duplicate paths; the file itself
   says so ("TODO: I've seen other dispatch_intent... Anything to DRY?" :2086). Both sync
   and async variants duplicate the `preferences.set` string-matched special case
   (:2011–2021 and :2093–2103) — a `match str` dispatch the project's own
   parse-don't-validate rule flags.
6. **Keybinding registry + hardcoded defaults** — seven bindings baked into the
   constructor (:1226–1254). Config data living in an engine ctor.
7. **BuilderServices facade** — 30+ trait methods (:1931–2277) forwarding to session,
   profiles, ui_state, provider cache, cell registry.

Plus embedded **debug tooling**: ~150 lines of diagnostics inside the per-block watcher
loop (:1556–1712), including a hardcoded `"block:default-main-panel"` block-ID check
(:1557, :1680) and an uncached `std::env::var("HOLON_TRACE_BLOCK_DATA")` **per CDC event**
(:1641) — one syscall per event on the hot path (a cached `LazyLock` copy of the same var
exists 45 lines later at :1686; the first check predates it and was never migrated).

The god-class is mirrored by a **god-trait**: `BuilderServices` (54–404) mixes
interpretation, watching, focus, dispatch, editing cells, link search, keybindings,
viewport, and runtime-handle access into one 30-method capability surface — every stub
implementor pays for all of it.

### The two `unimplemented!()` trait defaults

- `clone_arc` — `reactive.rs:70–76` (`unimplemented!("clone_arc not supported ...")`).
- `set_widget_open` — `reactive.rs:156–159` (`unimplemented!("BuilderServices::set_widget_open")`).

**Verdict: removable debt, not load-bearing.** Both are overridden by every real impl;
the defaults only exist so stubs compile without writing two lines. That trades a
compile-time obligation for a runtime panic — exactly the class of failure Rust's trait
system exists to prevent. Make both **required**; the stubs override anyway
(`StubBuilderServices`, ref-state mock — noted in the docs at :154–155). Same criticism
applies to the *panicking* defaults of `watch_block_signal` (:336), `watch_live` (:345),
`watch_query_live` (:369): a `WatchServices` sub-trait implemented only by the engine
would delete all three panics (see §4).

### The post-construction OnceLock self-slot

- `ReactiveEngine.services_slot: Arc<OnceLock<Arc<dyn BuilderServices>>>` (:1213),
  `BuilderServicesSlot` DI newtype (:2429), `make_interpret_fn` (:2628–2638),
  `clone_arc` reading it with `.expect(...)` (:1936–1941).
- Populated in `frontends/gpui/src/lib.rs:1208–1220` — where the double-set error is
  swallowed: `services_slot.set(services).ok();` (lib.rs:1220). Contra fail-loud.

**Verdict: load-bearing today, structurally removable.** The cycle exists *only because
the engine implements its own services trait*: engine needs `interpret_fn`, interpret_fn
needs `Arc<dyn BuilderServices>`, and that Arc IS the engine. Two clean exits:
(a) `Arc::new_cyclic` at construction (works since the slot is only read post-boot), or
(b) — better — split a thin `EngineServices(Arc<EngineCore>)` facade struct off the
engine; then construction is `core = Arc::new(EngineCore::new(...)); services =
Arc::new(EngineServices(core.clone()))` and the OnceLock, `BuilderServicesSlot`,
`make_interpret_fn`, and the panicking `clone_arc` default all delete. Illegal state
("engine exists but slot unset") becomes unrepresentable.

---

## 3. Correctness / perf hazards

### H1 — Watcher refcount leak (correctness, HIGH confidence)

`ensure_watching` (:1521) **increments refcount on every call** (:1525–1527), and it is
called from `watch_signal` (:1298), `watch_data_signal` (:1317), `watch_snapshot_stream`
(:1345), `watch_live` (:1380), **`snapshot_reactive` (:1514)**, **`get_block_data`
(:1945)** and **`await_ready` (:2198)**. But the only `unwatch` caller in the tree is
`ReactiveShell::drop` (frontends/gpui/src/views/reactive_shell.rs:855) — once per shell.
Every MCP `describe_ui`, PBT assertion, TUI snapshot, or builder `get_block_data` call
permanently inflates the count, so `unwatch`'s `refcount == 0` (:1863) is effectively
unreachable for any block ever snapshotted: the tokio watcher task, its CDC stream, and
the `ReactiveRenderedRows` leak for the app's lifetime, and a closed shell keeps
receiving+applying CDC for a block nobody renders. Fix is typestate, not discipline:
return an RAII `WatchGuard` from the counting path and give read-only paths a
non-counting `peek` (see §4).

### H2 — Full-doc resample per commit: yes, and it's structural for three of four consumers

The latency work's "projection resample ~83% of keystone wall" is this file:

- `reactive_signal[_with_ui_gen]` (:768/:782) re-runs `interpret_fn(expr, ALL rows)` on
  every emission, and its `data_signal` input (:570) is "full row set whenever any row
  changes". A CDC batch of N row changes = N inner-cell writes; `map_ref` will re-poll and
  fully reinterpret for whichever of those it observes (futures-signals coalesces under
  poll pressure, but a fast consumer sees close to per-row full-tree interprets).
- **GPUI root pump** uses exactly this path (`watch_signal`, frontends/gpui/src/lib.rs:1030)
  — the doc comment above it (lib.rs:1012-1014) claims it fires only on structural/viewport
  changes, but `reactive_signal_with_ui_gen` also folds `data_signal` in: any data change to
  the root-layout rows rebuilds root_vm + reconciles + renders the whole window.
- **watch_snapshot_stream** (:1341) additionally hashes focus into the ui-gen (:1348–1358),
  so the web/PBT snapshot pipeline reinterprets the entire tree per row change AND per
  focus move. This is the keystone's 83%.
- Only the `watch_live` path (:1375) escapes: `structural_signal_with_ui_gen` (:1412) +
  per-row `ReactiveView` drivers. The incremental machinery exists; the root pump and all
  snapshot consumers simply don't sit on it. So: **structural for snapshot consumers as
  currently shaped, but not inherent** — the fix is making snapshot consumers diff at the
  ReactiveView layer (or memoize `interpret_row` keyed by `(row-arc-ptr, expr-gen)`),
  not a new pipeline.

### H3 — Torn reads (MEDIUM, mostly by design but unfenced)

- `ReactiveRenderedRows::snapshot` (:742–745) reads `render_expr` then `rows` as two
  separate lock acquisitions — a Structure event between them yields new-expr/old-rows.
  `apply_event` deliberately does NOT clear rows on Structure (:678–680, "avoiding flash
  of empty content"), so every consumer of `snapshot()`/`get_block_data` can interpret a
  new expr against the previous generation's rows for a window. Disclosed nowhere at the
  read site; PBT settle loops paper over it.
- First-batch apply-then-retain (:704–731): between applying the new snapshot rows and
  `retain_keys`, observers of `data_signal` can see the *union* of old and new query
  results. Intentional trade (cell-identity preservation, :698–703) but again observable.
- `set_focus_with_caret` (:1052) writes `pending_caret_seed` then `focused_block` — two
  Mutables, non-atomic; and the `focused_occurrence` spike (:980) is a third parallel
  Mutable that can legally hold `Some(n)` while `focused_block` is `None`. Illegal state
  representable; see §4 FocusState.
- `apply_change`'s generation check (:459) is check-then-act against a separately-written
  `Mutable<u64>`; benign today only because one watcher task is the sole writer per set.
  Three generation notions coexist (`ReactiveRowSet.generation`,
  `ReactiveRenderedRows.data_generation` AtomicU64 :632, `UiState.viewport_generation`) —
  a `Generation` newtype threaded through would collapse the ambiguity.

### H4 — Silent drops & swallowed errors (project-rule violations)

- `FieldsChanged` for a row not in the set is **silently ignored** (:500–508, no else
  branch) — data loss without a trace line, contra "Fail Loud, Never Fake".
- Created-for-existing-row degraded to Updated with a "Defensive:" comment (:468–471) —
  explicitly the pattern CLAUDE.md bans; if this happens it's an upstream CDC bug that
  should at least warn.
- `watch_ui`/`watch_query` task startup failure → `tracing::warn!` and the block silently
  renders as eternal "loading" (:1715–1717, :1823–1825). No error surfaces in the UI.
- `services_slot.set(...).ok()` (gpui lib.rs:1220).
- `watch_query` sync bridge (:1991–1995): scoped-thread + `block_on` + `.join().unwrap()`
  — converts a worker panic into an opaque unwrap panic, and spawns an OS thread per call
  on a builder path.

### H5 — Hot-loop diagnostics
Covered in §2: per-event `env::var` (:1641), hardcoded block-id tracing (:1557), ~150
lines of triage scaffolding living permanently in the watcher loop. Cheap to extract,
real per-event cost, and it buries the 6 lines of actual logic (`reactive.apply_event(event)`).

---

## 4. Proposed decomposition

Target shape — `crates/holon-frontend/src/engine/` module with the god-file split into:

| New module | Contents (moved from reactive.rs) | Key type changes |
|---|---|---|
| `engine/row_store.rs` | `ReactiveRowSet` (:430), `ReactiveRenderedRows` (:621) | `Generation(u64)` newtype; `apply_event` takes `UiEvent` parsed to an internal enum that makes "Data before Structure" unrepresentable |
| `engine/watch_pool.rs` | `ReactiveRegistry` (:890), `WatcherState` (:917), `ensure_watching` (:1521), `ensure_query_watching` (:1786), `unwatch` (:1858), `set_variant` (:1844) | **`WatchGuard` RAII**: counting acquisition returns a guard whose Drop decrements; `peek(&EntityUri) -> Option<Arc<ReactiveRenderedRows>>` for snapshot/get_block_data paths (no count). Manual `refcount: usize` deleted — leak class (H1) gone by construction |
| `engine/watch_diag.rs` | the :1556–1712 diagnostics as one `fn trace_event(bid, &event, &rows)` behind `tracing::enabled!` | env var read once via `LazyLock` |
| `engine/ui_state.rs` | `UiState`, `ViewportInfo` | **`FocusState { block: EntityUri, occurrence: Option<u32>, caret_seed: Option<CaretSeed> }` in ONE `Mutable<Option<FocusState>>`** — kills the 3-Mutable desync (H3); `viewport: Mutable<Option<ViewportInfo>>` stays, generation derived from it |
| `engine/dispatch.rs` | `dispatch_intent`, `dispatch_intent_sync`, `dispatch_intent_chain`, `maybe_mirror_*`, `apply_structural_focus`, the preferences special case | ONE `IntentDispatcher { session, runtime, focus }` with `async fn run(intent)`; fire-and-forget = `spawn(run)`. Preferences special case becomes a parsed `Intent::SetPreference` variant at the intent boundary (parse-don't-validate), not string matching in two places |
| `engine/keybindings.rs` | `key_bindings` + defaults | defaults loaded from config/const table, not ctor statements |
| `engine/services.rs` | `impl BuilderServices` as `EngineServices(Arc<EngineCore>)` | deletes `services_slot`, `BuilderServicesSlot`, `make_interpret_fn`'s OnceLock, and the `clone_arc` panic default |
| `engine/mod.rs` | slim `EngineCore { session, runtime, interpreter, watch_pool, ui_state, dispatcher, key_bindings, provider_cache, cell_registry }` + `watch_*`/`snapshot` composition | `block_cell_registry: Mutex<Option<...>>` → set at construction (builder pattern), Option removed from steady state |

Split `BuilderServices` (54–404) into role traits with a blanket supertrait for existing
call sites: `InterpretServices` (interpret, resolve_profile, profile_signal, ui_state,
viewport, key_bindings), `FocusServices` (focused_block*, set_focus*, caret seed),
`DispatchServices` (dispatch_intent*, present_op), `WatchServices` (get_block_data,
watch_*, unwatch, await_ready, snapshot_resolved), `EditServices` (editable_text,
search_link_candidates). Stubs implement only what they need; the five panicking/
`unimplemented!()` defaults become compile errors where they belong.

### Migration order (de-risk first, per refactor doctrine)

1. **Experiment / de-risk (small, independently landable):** add `WatchGuard` + `peek` in
   place, convert `snapshot_reactive`/`get_block_data`/`await_ready` to `peek`, and add a
   regression test: N snapshots then shell-drop → watcher actually aborts. This fixes H1
   (a live prod-bug candidate) before any file moves, and proves the pool boundary.
2. Extract `watch_diag.rs` + `keybindings.rs` (pure code motion, zero behavior).
3. Extract `ui_state.rs` with `FocusState` unification; the ~10 readers ADR 0010 already
   enumerates are the checklist. This is the highest type-safety payoff.
4. Extract `dispatch.rs`; collapse the three dispatch paths onto one async core
   (dispatch_intent = spawn(sync)); parse `preferences.set` at the boundary.
5. Introduce `EngineServices` facade; delete OnceLock slot machinery; `clone_arc` becomes
   `self.0.clone()` — trivially correct.
6. Split the trait into role traits (mechanical, blanket impl keeps call sites green);
   make `set_widget_open`/`clone_arc` required.
7. **Perf (separate track, after 1–6):** move the GPUI root pump and
   `watch_snapshot_stream` off `reactive_signal` onto structural+per-row (or memoized
   `interpret_row`) — this is the H2/83% keystone lever and deserves its own
   measured PR against the latency instrumentation.

Per the doctrine: no old paths left behind — `reactive.rs` ends as a re-export shim for
one release of the branch, then deletes.

---

## Appendix: supporting evidence pointers

- God-class TODO: reactive.rs:1186. `unimplemented!()` defaults: :71, :158.
- OnceLock slot: :1213, :2429, :2628; set-site swallow: frontends/gpui/src/lib.rs:1220.
- Full-reinterpret warning in-file: :766. Root pump: frontends/gpui/src/lib.rs:1030.
- Focus-hash snapshot stream: :1348–1358. Live path escape hatch: :1375–1429.
- Refcount ++ everywhere: :1298, :1317, :1345, :1380, :1514, :1945, :2198; only
  decrement: reactive_shell.rs:855.
- Silent FieldsChanged drop: :500–508. Defensive Created-as-Updated: :468–471.
- docs/Architecture/RenderPipeline.md "Reactive layer" section matches the watch_live
  path but not the root/snapshot paths — doc understates the full-resample consumers.
