# Fixing Notification PBT Failures

## What We Observed
- The stateful property test compares `MemoryBackend` (reference) with `LoroBackend` (SUT).
- `ReferenceState` and `BlockTreeTest` both keep notification buffers inside `Arc<Mutex<Vec<BlockChange>>>`.
- Because `ReferenceState` derives `Clone`, every clone produced by `proptest` shares the same `Arc`. Events recorded while generating or shrinking transitions leak into the clones used during execution. Before the first transition we already observe mismatched notification counts.
- The SUT side compensates by calling `emit_change` directly, bypassing `watch_changes_since` entirely. That means the test no longer exercises the production API.
- `LoroBackend::watch_changes_since` ignores the supplied version and simply replays an in-memory log. It never talks to the Loro CRDT stream and therefore cannot serve clients that resume from arbitrary versions.

## Root Causes
1. **Reference state is not pure data.**
   `ReferenceState` spawns a background task in `Default::default()` and stores the stream handle plus shared buffer inside the state. Cloning the state copies the `Arc`, so all clones observe the same mutable data.
2. **`watch_changes_since` is a stub.**
   The implementation only replays an ephemeral `event_log`, making it impossible to resume from a specific version or to validate version handling in tests.
3. **PBT harness sidesteps the API.**
   By calling `emit_change`, the harness no longer validates the behaviour of `watch_changes_since`.

## Revised Plan

### 1. Purify state while keeping expectations model-driven
- Implement manual `Clone` for `ReferenceState` that performs deep copies of its data and avoids cloning any async machinery.
<!-- I think/hope we don't need `Clone` for `BlockTreeTest` as no shrinking should be performed on it -->
- Remove background subscriptions from the state itself. Model notification retrieval as explicit state-machine commands while leaving expectations derived solely from executing the reference backend.
- Track backend versions in the reference state (`Vec<Vec<u8>>`). Maintain a parallel version map for the SUT so we can translate between memory versions and Loro versions at runtime.

### 2. Support version-aware notifications in both backends
- Extend MemoryBackend to honour `watch_changes_since(version)` for arbitrary versions (e.g., append-only change log indexed by sequence number).
- For LoroBackend:
  - Subscribe to all relevant containers (`blocks_by_id`, `children_by_parent`, `root_order`, metadata).
  - Persist or reconstruct Loro updates so a delta between an arbitrary version vector and the current state can be produced.
  - On `watch_changes_since(version)`, replay backlog to the caller and then stream live updates.

### 3. Rebuild the property-based test around commands
- Extend the transition set with `WatchChanges` and `UnwatchChanges` commands. Each watch command captures the relevant versions, attaches temporary subscribers, drains the backlog, and stores the version it advanced to. Unwatch tears down the subscription.
- Mutating commands continue to call backend operations and record the returned versions for later watch commands.
- Eliminate `std::thread::sleep`; await async tasks explicitly so the test remains deterministic.

### 4. Implementation considerations
- Introduce a lightweight watcher registry that is *not* cloned. Reference/SUT states only keep metadata necessary to rebuild watchers.
- Ensure state cloning copies data only (blocks, IDs, versions, expected notifications).

## Subscription-management options snapshot

| Approach | Summary | Pros | Cons |
| --- | --- | --- | --- |
| **A. Pure data only** | No live watchers; expectations computed synchronously from reference backend data. | Simplest clone semantics, zero async code. | Never hits `watch_changes_since`, so we re-implement notification logic in the harness; hard to model live interactions. |
| **B. Runtime registry** | Reference state holds metadata, while a per-run registry owns actual subscriptions. | State stays pure data; real API exercised; easy to rebuild for shrinking. | Extra infrastructure (registry lifecycle, teardown) to maintain. |
| **C. Clone-rebuild watchers** | `Clone` rebuilds watchers by re-subscribing on the cloned backend and replaying backlog to the stored version. | Clone contract stays intuitive; no registry required; still uses real API. | `Clone` becomes complex (block_on async work, resource management); cloning cost grows with number of watchers/backlog size. |

### Option B sketch (runtime registry)
```
WatchChanges command:
  - Reference state records watcher metadata (id, base version, etc.)
  - Registry creates real subscriptions on both backends
  - Registry drains backlog → returns Vec<BlockChange>
  - Test compares backlogs, stores last version

Mutating command:
  - Run operation on both backends
  - Record new versions in state
  - Registry drains pending events for each active watcher

UnwatchChanges command:
  - Reference state removes metadata
  - Registry cancels watcher tasks and drops handles
```

### Option C sketch (clone-rebuild)
```
ReferenceState clone:
  - Copy pure data (blocks, versions, watcher descriptors)
  - For each watcher descriptor:
      stream' = backend_clone.watch_changes_since(last_version)
      drain backlog until caught up
      store stream' + descriptor in clone
```

If we pursue option C, `Clone` must wrap its async work in `Runtime::block_on`, and we should document the cloning cost so future maintainers understand the contract.

#### Option C – deeper dive

**State layout**
- `ReferenceState` holds:
  - `backend: MemoryBackend` (or other pure-data cloneable backend)
  - `versions: Vec<Vec<u8>>` – snapshot versions after each command
  - `watchers: HashMap<WatcherId, WatcherDescriptor>`
- `WatcherDescriptor` is pure metadata:
  ```rust
  struct WatcherDescriptor {
      watcher_id: WatcherId,
      base_version_idx: usize,      // index into versions Vec
      last_consumed_idx: usize,     // index of most recent version seen
      pending_events: Vec<BlockChange>, // events already drained but not yet asserted
      live_stream: Option<WatcherStream>, // runtime handle, not cloned directly
  }
  ```
  The `WatcherStream` wrapper contains the actual subscription (`Pin<Box<dyn Stream<…>>>`) plus any runtime bookkeeping (tasks, handles).

**Clone algorithm**
1. `ReferenceState::clone` is synchronous. Inside it we create a small Tokio runtime (or reuse a handle carried in the state) and call `block_on` for any async work.
2. The backend is cloned normally (MemoryBackend supplies `Clone`).
3. For each `WatcherDescriptor`:
   - Look up the byte-version corresponding to `last_consumed_idx` and pass it to `backend_clone.watch_changes_since`.
   - Drain the returned stream until it yields no backlog or until we have reproduced `pending_events`. Any events fetched during clone that were not part of `pending_events` become the new `pending_events` for the clone (usually empty if the original watcher was fully drained).
   - Store the new stream handle inside the cloned descriptor so the watcher remains “live” after cloning.
4. Assemble the cloned state with cloned backend, versions vector, and cloned watcher descriptors (each with fresh `live_stream`s).

**Runtime behaviour**
- When a `WatchChanges` command executes on the *original* state:
  1. Capture the version index to start from (usually `versions.len() - 1`).
  2. Call `watch_changes_since` on both backends and drain the initial backlog; append resulting events into the descriptor’s `pending_events`.
  3. Store the stream handle in `WatcherDescriptor.live_stream`.
- When a mutating command runs:
  1. Execute the operation on both backends and push the returned versions into `versions` (reference) and `sut_versions` (SUT).
  2. For every watcher with a live stream, synchronously pull pending events (e.g., via `block_on(stream.next())` in a loop until it would block). Append new events into `pending_events` and update `last_consumed_idx`.
- `UnwatchChanges` simply drops the descriptor’s stream (allowing the subscription to close) and removes it from the map.

**Handling async inside `Clone`**
- Because `Clone` must be synchronous, every call to `watch_changes_since` and draining of streams is wrapped inside `runtime.block_on`.
- To avoid creating a new runtime for every clone we can keep a `tokio::runtime::Handle` in the state and use `Handle::block_on`—but that in turn must be clone-safe (`Handle` implements `Clone`).
- Any failure to rebuild a watcher (e.g., backend error) should panic rather than propagate, aligning with `Clone`’s usual semantics (if we can’t clone, treat it as a test failure).

**Pros revisited**
- Clones remain self-contained; we can hand them to shrinking without referencing global registries.
- Watchers always hit the real API.
- No extra registry or command infrastructure is required beyond storing descriptors.

**Cons / caveats**
- `Clone` now depends on backend behaviour. If `watch_changes_since` becomes heavier, cloning cost scales accordingly.
- Multiple watchers multiply the cost because each clone spins up fresh streams and drains them.
- We must ensure that dropping the cloned state tears down all watcher streams to avoid leaked subscriptions.
- Difficult to share the same watcher logic between MemoryBackend and LoroBackend unless both expose identical cloning semantics.

**Sequence diagram (simplified)**

```
Original state         Clone runtime                  Cloned backend
     |                        |                            |
     |--- clone() ----------->|                            |
     |                        |-- block_on(clone_async) -->|
     |                        |                            |-- watch_changes_since(Vn) --> stream'
     |                        |<--------------- stream' ---|
     |                        |-- drain backlog ---------->|
     |                        |                            |
     |                        |-- store stream' ---------->|
     |<-- new ReferenceState -|                            |
```

This option is viable if we’re comfortable with heavier clone semantics and we want each clone to be a drop-in replacement of the original, including live watchers.

## Questions to Resolve
1. **Loro delta retrieval:** What is the best way to compute “updates since version”? Research Loro’s update export/import APIs to choose between replaying updates, using checkpoints, or persisting CRDT snapshots.
2. **Version identity mapping:** Decide on a stable identifier (sequence number, hash) to map reference versions to SUT versions so we can compare notifications command-by-command.
3. **Subscription coverage:** Confirm the set of Loro containers we must monitor so that all block tree changes are reflected in notifications.

## Next Steps
1. Implement manual `Clone` impl for `ReferenceState` according to Option C.
2. Implement the watcher registry (shared between Memory and Loro backends) and add `WatchChanges` / `UnwatchChanges` commands to the state machine.
3. Update both backends to produce real version-aware `watch_changes_since` streams.
4. Re-enable the property test and iterate until it passes consistently, validating both structural equality and notification behaviour. We'll rely on the PBT as the primary regression net.
