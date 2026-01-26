# Headless shadow-index subscription topology (Phase B0 design note)

Context: plan `deep-humming-crane.md` Phase B3 needs the headless
`DirectMutationDriver` / `ReactiveEngineDriver` to own a persistent
`IncrementalShadowIndex` that is patched incrementally, matching how
production GPUI uses the index. This note documents how production does
it so B3 can mirror the same topology.

## Production pattern (GPUI)

The shadow index is a **single shared** `Arc<RwLock<Option<IncrementalShadowIndex>>>`
owned by `NavigationState` (`frontends/gpui/src/navigation_state.rs:12`).
Every `ReactiveShell` (`frontends/gpui/src/views/reactive_shell.rs`)
holds a clone of the same `NavigationState`, so all shells patch the
same flat index.

Each shell represents one block. When a shell is constructed or its
`structural_changes` stream fires, it calls `reconcile_children`
(`reactive_shell.rs:273`), which:

1. Runs `walk_for_entities` (`:780`) on its reactive tree to collect the
   **direct** child block_ref IDs. `walk_for_entities` **stops at
   BlockRef** — it does not recurse into their content. Each shell only
   knows about its immediate BlockRef children, not grandchildren.
2. Reconciles `child_block_refs: HashMap<String, Entity<ReactiveShell>>`
   against the set from step 1 — creates new nested shells for newly
   discovered BlockRefs (via `reconcile_block_ref_entities` at `:379`),
   drops shells for BlockRefs that have left the tree.
3. Calls `resolve_snapshot(cx)` (`:186`), which `snapshot_resolved`'s
   the current reactive tree using a closure that recurses into
   `child_block_refs` to inline nested block content (`:189-196`).
   Produces a fully-resolved `ViewModel` for the shell's own block
   **plus** every nested block_ref's content.
4. Calls `self.nav.patch_shadow_block(own_block_id, &resolved, &focus)`
   (`:331-335`), which internally calls `IncrementalShadowIndex::patch_block`.

Cycle prevention is a thread-local `RECONCILING` set (`:286-325`) that
tracks which block_ids are currently reconciling up the call stack;
self-references and A→B→A cycles are filtered out before creating
nested shells.

## Observations

- **One index, many patchers.** Each block patches its own range
  independently. There is no coordinator — the `Arc<RwLock<…>>` is the
  coordination point.
- **Redundancy today.** Because `resolve_snapshot` recursively inlines
  nested block content, an outer shell's patch writes over the same
  range that an inner shell's patch covers. Both write equivalent flat
  content (modulo race), so it's idempotent but quadratic across levels
  of nesting. This is inherited from the current `&ViewModel`-based
  `IncrementalShadowIndex` API — the index only knows how to flatten a
  single resolved `ViewModel`, so whoever calls `patch_block` must
  inline everything they own.
- **Cadence.** Patches run: (a) initial construction in `new_for_block`,
  (b) every `structural_changes` emission (`:82-103`), (c) every inner
  collection `VecDiff` via `subscribe_inner_collections` (`:351-377`).

## Implication for Phase B1

B1 changes `IncrementalShadowIndex::build`/`patch_block` to take
`&ReactiveViewModel` and has `flatten_recursive` **stop at `ReactiveViewKind::BlockRef`
boundaries** — the same stop `walk_for_entities` already uses. Under
this model:

- An outer shell's patch covers *only* its own content down to each
  BlockRef stop. It does not inline inner content.
- Each inner shell independently patches its own range.
- There is no overlap, no redundancy, no quadratic inlining.
- Production `reactive_shell.rs:186-196` `resolve_snapshot` becomes
  unnecessary for the shadow-index path — the shell can pass its
  `current_tree: ReactiveViewModel` directly to `patch_shadow_block`.
  (`resolve_snapshot` remains in use at `lib.rs:137-154` for other
  purposes, so it stays in the codebase.)

This is a net simplification of prod. B1 lands both simplifications
together.

## Headless driver design (B3)

Mirror the production pattern exactly:

```rust
struct HeadlessShadowRouter {
    index: Arc<Mutex<Option<IncrementalShadowIndex>>>,
    subs:  Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    engine: Arc<ReactiveEngine>,
    settled: Arc<tokio::sync::Notify>,
    reconciling: Arc<Mutex<HashSet<String>>>, // cycle guard
}
```

Behaviour:

- On driver creation, subscribe to the root layout block via
  `engine.watch(&root_id)` → `Stream<ReactiveViewModel>`.
- Each emission for block B runs a `reconcile_block` pass:
  1. Walk the emission with `walk_for_entities` (or its headless
     equivalent) to find direct BlockRef children.
  2. For each new child ID, spawn a drain task that subscribes to
     `engine.watch(&child_uri)` and runs the same `reconcile_block`
     pass on every emission. Record the handle in `subs`.
  3. For each child ID that left the tree, abort + remove its handle.
  4. `patch_block(B, &rvm)` under the shared lock. This only covers B's
     own range down to BlockRef stops (post B1).
  5. `settled.notify_waiters()`.
- Cycle guard: before spawning a child subscription, check
  `reconciling`; skip if the child ID is already active up the stack.
  Drop a guard that removes it on exit.
- `send_key_chord` dispatches the mutation, then awaits
  `settled.notified()` so PBT invariants observe a patched index.

## H4/H5/H6 status after this note

- **H4** (topology): resolved. Walk-and-subscribe per direct child,
  exactly like `walk_for_entities` + `reconcile_block_ref_entities`.
- **H5** (async drain sync): addressed by the `settled: Notify` gate in
  the design above.
- **H6** (enigo parallelism): unchanged — scoped to GPUI-backed PBTs
  only, not the headless path this note covers.
