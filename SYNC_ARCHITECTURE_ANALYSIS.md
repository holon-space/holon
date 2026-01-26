# Sync Architecture Analysis: Turso / Loro / Iroh / Org Files

## 1. System Overview

Four stores must stay convergent:

```
                    +-----------+
                    |  Org File |  (human-readable, on disk)
                    +-----+-----+
                          |
              OrgAdapter  |  OrgFileWriter
              (file→Loro) |  (Loro→file)
                          |
                    +-----+-----+
                    |   Loro    |  (CRDT, in-memory + .loro snapshots)
                    +-----+-----+
                          |
           LoroEventAdapter  |  CacheEventSubscriber
                          |
                    +-----+-----+
                    |   Turso   |  (SQL cache, CDC for UI)
                    +-----+-----+
                          |
                    IrohSyncAdapter
                          |
                    +-----+-----+
                    |    Iroh   |  (P2P sync)
                    +-----------+
```

The system claims **Loro is source of truth**. All mutations flow through Loro, which emits
events via `LoroEventAdapter → TursoEventBus`. Org files are a "derived view" rendered by
`OrgFileWriter`. External org edits are ingested by `OrgAdapter` into Loro.

## 2. The Five Data Flows

### Flow A: External Org Edit → Loro (OrgAdapter)

```
User edits .org file
  → FileWatcher detects change
  → OrgAdapter.on_file_changed(path)
  → WriteTracker.is_our_write(path)?  →  if true: STOP
  → parse_org_file() → Vec<Block>
  → diff with known_state (HashMap<block_id, Block>)
  → for new blocks:     command_bus.create(block)    → Loro upsert
  → for changed blocks: command_bus.update(block)    → Loro upsert
  → for deleted blocks: command_bus.delete(block_id) → Loro delete
  → update known_state = parsed blocks
```

### Flow B: Loro → Org File (OrgFileWriter)

```
Loro mutation happens (from OrgAdapter, UI, or Iroh)
  → LoroBlockOperations emits Change<Block> via broadcast channel
  → LoroEventAdapter converts to Event(origin=Loro)
  → TursoEventBus publishes to events table
  → OrgFileWriter subscription receives event
  → debounce 500ms (reset on each new event)
  → render_all_documents():
      for each loaded LoroDocument:
        blocks = backend.get_all_blocks()
        org_content = OrgRenderer::render_blocks(blocks)
        content_hash = hash(org_content)
        disk_hash = hash(file on disk)
        if content_hash == disk_hash → SKIP (idempotency)
        else → WriteTracker.mark_write_with_hash(path, content_hash)
             → write org_content to disk
```

### Flow C: Loro → Turso Cache (CacheEventSubscriber)

```
LoroEventAdapter publishes Event(origin=Loro)
  → CacheEventSubscriber receives (no origin filter)
  → tokio::spawn(async { cache.apply_batch(change) })  // spawned to avoid deadlock
  → Turso CDC fires → UI stream updates
```

### Flow D: Loro ↔ Iroh (P2P sync)

```
IrohSyncAdapter.sync_with_peer(doc, peer):
  → send our Loro snapshot
  → receive peer's snapshot
  → doc.apply_update(peer_snapshot)  // CRDT merge
  → Loro emits changes → triggers Flows B and C
```

### Flow E: Startup Initialization

```
1. Turso schema created (DDL)
2. LoroDocumentStore loads .loro snapshots from disk
3. LoroEventAdapter starts (listens to broadcasts)
4. OrgFileWriter starts (subscribes to EventBus, origin=Loro)
5. OrgAdapter.start_event_subscription() (subscribes to EventBus, origin=Loro)
6. OrgFileWatcher starts → scans all .org files → triggers OrgAdapter.on_file_changed()
7. OrgAdapter processes each file → sends to Loro → Loro emits events
8. OrgFileWriter renders → content-hash check → maybe writes
```

## 3. Loop Prevention: Three Layers

| Layer | Mechanism | What It Prevents |
|-------|-----------|-----------------|
| **L1: Origin filtering** | OrgFileWriter subscribes only to `origin=Loro` events | OrgAdapter's `origin=Org` events don't trigger OrgFileWriter |
| **L2: Content-hash idempotency** | `hash(render(Loro_state)) == hash(disk_content)` → skip write | Loro synced from org file → renders identical content → no write |
| **L3: WriteTracker** | `mark_write_with_hash(path, hash)` before write; `is_our_write(path)` checks hash + 2s window | FileWatcher fires on our write → OrgAdapter skips reprocessing |

**The layers work together**: L1 prevents direct echo. L2 prevents unnecessary writes. L3 prevents
re-ingestion of our own writes.

## 4. Identified Failure Modes

### FM1: Startup Stale Overwrite (CRITICAL)

**Scenario**: Loro snapshot has NEWER content than org file (e.g., from P2P sync while app was off, or from a previous session's UI edit that wasn't written to org due to crash).

```
1. .loro snapshot loaded with blocks {A: "new content"}
2. .org file has {A: "old content"}
3. FileWatcher scans → OrgAdapter.on_file_changed()
4. OrgAdapter: known_state is EMPTY (no events received yet)
5. OrgAdapter: block A appears as "new" → sends create(A, "old content") to Loro
6. Loro: upsert → OVERWRITES "new content" with "old content"
7. P2P-synced / UI-edited content is LOST
```

**Root cause**: OrgAdapter has no way to know that Loro's version is newer. It always
sends the org file's content. The `known_state` is empty at startup because the EventBus
subscription hasn't delivered any events yet.

**Why this keeps coming back**: Every "fix" that addresses the symptom (e.g., delaying
OrgAdapter, preloading known_state) doesn't address the root cause: **there is no version
ordering between stores**.

### FM2: Debounce Window Partial Render (CRITICAL)

**Scenario**: OrgAdapter takes longer than 500ms to process a large file change, with gaps
between block operations.

```
1. OrgAdapter starts processing file with 50 blocks
2. Creates blocks 1-10 → Loro events fire
3. Document resolution for block 11 takes 600ms (IO)
4. OrgFileWriter debounce expires (500ms since last event)
5. OrgFileWriter renders from Loro: has blocks 1-10, plus OLD blocks 11-50
6. Writes partial/stale content to org file
7. WriteTracker marks this write
8. OrgAdapter resumes, creates blocks 11-50
9. But meanwhile, the org file has been overwritten with partial content
10. If OrgAdapter re-reads the file (e.g., another file change), it sees partial content
```

**Root cause**: The debounce assumes all related mutations arrive within 500ms. The
`external_processing` mechanism (which paused OrgFileWriter during OrgAdapter processing)
was removed.

### FM3: Normalization-Driven Loop

**Scenario**: OrgRenderer produces slightly different output than what was in the org file
(whitespace, property ordering, quoting differences).

```
1. Org file has: "* TODO [#A] Headline   :tag1:tag2:"
2. OrgAdapter parses → sends to Loro
3. OrgFileWriter renders from Loro: "* TODO [#A] Headline :tag1:tag2:"  (one space, not three)
4. Content hash differs → writes to org file
5. FileWatcher fires → WriteTracker catches it → OK
6. But WriteTracker window expires → next file scan re-processes
7. OrgAdapter: blocks_differ() may detect differences due to normalization
8. Sends update to Loro → Loro emits event → OrgFileWriter renders again
9. Content matches this time → STOP (usually)
```

**Root cause**: `parse(render(block))` is not a perfect roundtrip. Normalization differences
between parser and renderer cause spurious changes.

### FM4: known_state Race with Event Subscription

**Scenario**: A UI mutation and file edit happen concurrently.

```
1. UI creates block X in Loro at t=0
2. LoroEventAdapter publishes event at t=1
3. OrgAdapter event subscription receives event at t=5 → updates known_state
4. But at t=2, user edits org file (deletes block X)
5. OrgAdapter.on_file_changed() at t=3 → reads known_state (doesn't have X yet)
6. X is not in known_state → not detected as "deleted"
7. Block X persists in Loro despite being removed from org file
8. Eventually, known_state catches up → but the org file processing already happened
```

**Root cause**: `known_state` update via EventBus is async. OrgAdapter can process a file
change before the event subscription delivers all relevant events.

### FM5: blocks_differ() Incomplete

`blocks_differ()` does NOT compare:
- `source_language` (e.g., changing `#+begin_src holon_prql` to `#+begin_src holon_sql`)
- `source_name` (e.g., changing `#+name: old` to `#+name: new`)
- `_source_header_args` (e.g., changing `:connection` parameter)

**Impact**: External edits to these fields silently fail to sync to Loro.

### FM6: Concurrent CacheEventSubscriber Out-of-Order Application

CacheEventSubscriber spawns a new `tokio::spawn` for each event to avoid deadlock:

```rust
tokio::spawn(async move {
    cache.apply_batch(&[change], None).await;
});
```

Events can be applied out of order. A `block.updated` event might arrive before
`block.created`. The UI would temporarily show incorrect state.

## 5. Required Invariants

### I1: Convergence (Eventually Consistent)

> After quiescence (all debounce windows expired, all events processed, no pending
> file changes), for every block B:
> `Loro(B) ≡ OrgFile(B) ≡ TursoCache(B)` (modulo fields not representable in each format)

**Currently**: Tested by E2E PBT, but only for sequential mutations. Not tested for
concurrent mutations with timing dependencies.

### I2: No Stale Overwrite

> If block B has version V₁ from source S₁ and version V₂ from source S₂, and V₂ is
> causally newer than V₁, then the converged state must reflect V₂.

**Currently**: NOT enforced. No causal ordering exists. OrgAdapter always overwrites
Loro with org file content regardless of version.

### I3: Write Loop Termination

> Any single external event (file edit, UI mutation, P2P sync) should produce at most
> one write cycle through the system before reaching quiescence.

**Currently**: Mostly enforced by the three-layer loop prevention, but can fail under
FM3 (normalization differences).

### I4: Processing Idempotency

> Processing the same org file content twice (without intervening changes) must not
> modify any state.

**Currently**: Enforced by `blocks_differ()` for most fields, but NOT for source block
metadata (FM5).

### I5: Deterministic Rendering

> `render(blocks₁) = render(blocks₂)` whenever `blocks₁ ≡ blocks₂`

**Currently**: Assumed but not tested. Failures cause FM3.

### I6: Roundtrip Preservation

> For blocks representable in org format: `parse(render(B)) ≡ B`

**Currently**: Partially tested by E2E PBT, but not isolated as a property test.

### I7: Write Count Bound

> The total number of org file writes should be ≤ 2 × number of actual mutations.

**Currently**: Not tested.

### I8: Startup Ordering

> OrgAdapter must have complete knowledge of Loro state before processing org files.
> OR: OrgAdapter must not overwrite Loro state that is newer than the org file.

**Currently**: NOT enforced. This is the root cause of FM1.

## 6. PBT Strategy

### Level 0: Assertion-Based Invariants (Zero-Cost)

Add assertions to production code that catch invariant violations immediately:

```rust
// In OrgAdapter.on_file_changed(), after diffing:
assert!(
    created.len() + updated.len() + deleted.len() <= known_blocks.len() + current_blocks.len(),
    "Impossible diff: more operations than blocks"
);

// In OrgFileWriter.render_all_documents():
assert!(
    org_content.contains(":ID:") || blocks.is_empty(),
    "Rendered org content has blocks but no :ID: properties"
);

// In LoroBlockOperations.create():
// After upsert, verify the stored block matches what we sent
let stored = self.get_block(&block.id).await?;
assert_eq!(stored.content, block.content, "Create/upsert silently dropped content");
```

### Level 1: Unit PBTs (per component, fast, isolated)

#### PBT-U1: OrgParser ↔ OrgRenderer Roundtrip

```
Property: parse(render(blocks)) ≡ blocks
Input: Vec<Block> (randomly generated, valid org structure)
Steps:
  1. org_text = OrgRenderer::render_blocks(blocks)
  2. parsed = parse_org_file(org_text)
  3. assert: for each block in blocks, ∃ parsed block with same id, content,
     content_type, source_language, task_state, priority, tags, scheduled, deadline
```

This catches: FM3 (normalization differences), I5, I6.

#### PBT-U2: blocks_differ() Completeness

```
Property: For every field F in Block that is org-representable,
          changing F produces blocks_differ() == true
Input: Block + field mutation (one field changed at a time)
Steps:
  1. original = random_block()
  2. for each field F in [content, parent_id, content_type, source_language,
     source_name, task_state, priority, tags, scheduled, deadline, string_properties]:
       mutated = original.clone().with_field(F, different_value)
       assert: blocks_differ(original, mutated) == true
```

This catches: FM5 (incomplete comparison).

#### PBT-U3: WriteTracker Correctness

```
Property: is_our_write() returns true iff we wrote the current content within the window
Input: Sequence of (mark_write, external_edit, is_our_write, sleep) operations
Invariant:
  - After mark_write(path, hash): is_our_write(path) == true (if content unchanged)
  - After external_edit(path): is_our_write(path) == false
  - After window expiry: is_our_write(path) == false
```

### Level 2: Component Integration PBTs (2-3 components, medium speed)

#### PBT-I1: OrgAdapter + Known State Consistency

```
Property: After processing a file change, known_state accurately reflects the org file
Input: Sequence of (write_org_file, process_file_change, inject_loro_event) operations
Steps:
  1. Generate sequence of operations
  2. After each operation, verify:
     known_state[file] == parse_org_file(file) ∪ injected_events
```

This catches: FM4 (known_state race).

#### PBT-I2: OrgAdapter + OrgFileWriter Two-Store Convergence

```
Property: After quiescence, org file content matches Loro state
Setup: Mock OperationProvider that stores blocks in-memory
Input: Interleaved sequence of:
  - External file edits (modify org file on disk)
  - Internal mutations (modify blocks in mock store, emit events)
Steps:
  1. Apply operation
  2. Wait for quiescence (debounce + WriteTracker window + margin)
  3. Assert: blocks in mock store ≡ blocks parsed from org file
```

This catches: FM1, FM2, FM3, I1, I3.

**Key insight**: This test should use a mock Loro/OperationProvider that records
all operations, so we can verify the SEQUENCE of operations, not just the end state.
This lets us detect issues like "OrgAdapter sent an update that was immediately
overwritten by OrgFileWriter."

#### PBT-I3: Concurrent Convergence

```
Property: Under concurrent external edits and internal mutations, the system converges
Input: Pairs of (external_edit, internal_mutation) applied concurrently
Steps:
  1. Apply both operations "simultaneously" (within 10ms)
  2. Wait for quiescence
  3. Assert: system has converged (both edits reflected or merged)
  4. Assert: write count ≤ 4 (bounded)
```

### Level 3: E2E PBTs (full system)

#### PBT-E1: Stale Startup Recovery (NEW, tests FM1)

```
Property: On startup, Loro's newer content is preserved, not overwritten by stale org files
Setup:
  1. Write org file with content "old"
  2. Create .loro snapshot with content "new" (simulating P2P sync)
  3. Start app
  4. Wait for convergence
Assert:
  - Loro has "new" content (not "old")
  - Org file has been updated to "new" content
  - OR: merge of "old" and "new" (if CRDT merge applies)
```

This is the MOST IMPORTANT missing test. It directly tests the primary failure mode.

#### PBT-E2: Restart Cycle Consistency (NEW)

```
Property: Multiple restart cycles don't degrade data
Input: Sequence of (mutation, restart) operations
Steps:
  1. Apply mutation (UI or external)
  2. Restart app
  3. Assert: all mutations preserved after restart
  4. Repeat N times
  5. Assert: final state = reference model
```

#### PBT-E3: Write Count Bound (NEW, tests I7)

```
Property: Number of org file writes is bounded
Input: N mutations (external edits + UI mutations)
Steps:
  1. Instrument OrgFileWriter to count writes
  2. Apply N mutations, waiting for convergence after each
  3. Assert: total_writes ≤ 2 * N
```

#### PBT-E4: Debounce Stress (NEW, tests FM2)

```
Property: Large file changes don't cause partial renders
Input: Org file with 100+ blocks, modified externally
Steps:
  1. Write large org file
  2. Process through OrgAdapter
  3. Assert: org file on disk matches COMPLETE Loro state
  4. Assert: no intermediate partial states written
```

### Level 4: Metamorphic Testing

#### MT-1: Commutativity

```
Property: Applying mutations A then B produces same result as B then A
         (for non-conflicting mutations on different blocks)
```

#### MT-2: Idempotency

```
Property: Applying mutation A twice produces same result as applying A once
```

## 7. Why Debouncing is the Wrong Pattern

The current architecture uses three timing-dependent mechanisms:

1. **Debounce** (500ms): OrgFileWriter waits 500ms after the last Loro event before rendering
2. **WriteTracker window** (2000ms): OrgAdapter ignores file changes within 2s of our writes
3. **Event subscription delay**: OrgAdapter's `known_state` is populated asynchronously

All three are **timing heuristics** — they work "most of the time" but break under load,
slow IO, or unexpected delays. They make the system:

- **Non-deterministic**: Behavior depends on wall-clock timing
- **Untestable**: PBTs can't reliably exercise timing edge cases
- **Fragile**: Any change to processing speed can cause regressions
- **Latent-buggy**: Failures are intermittent and hard to reproduce

The debounce in particular is doing double duty: it's both a **performance optimization**
(coalesce rapid writes) and a **correctness mechanism** (don't render partial state). These
concerns should be separated. Performance batching is fine; depending on timing for correctness
is not.

### What the research says

Tonsky's analysis of CRDT file sync (tonsky.me/blog/crdt-filesync/) and the crdt-over-fs
project (github.com/3timeslazy/crdt-over-fs) identify the core pattern:

> **The file is a projection. External edits are detected by diffing against the
> last-projected state, not against the CRDT's current state.**

Automerge's sync protocol uses **change hashes / heads** to track what each peer has seen.
Echo suppression is structural (based on content identity), not temporal (based on timing).

## 8. The Correct Architecture: Projection + Diff-Ingestion

### Core Idea

Replace the two separate components (OrgAdapter + OrgFileWriter) with a single
**OrgSyncController** that owns one piece of state per file: `last_projection`.

```
last_projection: HashMap<PathBuf, String>
```

`last_projection[file]` is the org content we last wrote to disk (or read from disk at
startup and confirmed matches Loro). This is the **diff base** for detecting external edits.

### Flow 1: External Org Edit (replaces OrgAdapter)

```
FileWatcher detects change to file.org
  → read disk_content = read(file.org)
  → if disk_content == last_projection[file.org] → STOP (our own write, or no change)
  → old_blocks = parse(last_projection[file.org])
  → new_blocks = parse(disk_content)
  → diff(old_blocks, new_blocks) → create/update/delete ops
  → apply ops to Loro (CRDT merge handles conflicts)
  → re-project: rendered = render(Loro.get_all_blocks())
  → if rendered != disk_content:
      write rendered to file.org    ← Loro merged concurrent changes
  → last_projection[file.org] = rendered (or disk_content if no write)
```

### Flow 2: Loro Mutation (replaces OrgFileWriter)

```
Loro changes (from UI, P2P sync, etc.)
  → rendered = render(Loro.get_all_blocks())
  → if rendered == last_projection[file.org] → STOP (no net change)
  → write rendered to file.org
  → last_projection[file.org] = rendered
```

### Why This Eliminates All Timing Dependencies

**No debounce needed for correctness**:
- Flow 2 can still batch writes for *performance* (e.g., coalesce 50 rapid block creates
  into one file write). But correctness doesn't depend on the batching — each intermediate
  state is valid, just potentially incomplete.
- The batching is purely a write-coalescing optimization: "don't write to disk more than
  once per 100ms." Not "wait 500ms and hope all mutations have arrived."

**No WriteTracker time window needed**:
- Echo suppression is `disk_content == last_projection`. This is a permanent comparison,
  not time-windowed. It works whether the FileWatcher fires 10ms or 10 seconds after our write.

**No async known_state needed**:
- The diff base is `last_projection`, which is always available synchronously (it's
  the content we ourselves produced). No need to wait for EventBus events to populate
  a separate known_state.

**No startup race**:
- At startup: render from Loro → set as `last_projection`. Then when FileWatcher scans
  files, external edits are correctly detected as `disk_content != last_projection`
  (because `last_projection` came from Loro, reflecting its current state).

### Handling the Startup Problem (FM1)

The critical question: at startup, Loro has state X, org file has state Y. Which wins?

With projection + diff-ingestion:

```
1. Startup: load Loro → render → last_projection = render(Loro)
2. FileWatcher: read disk_content from org file
3. If disk_content == last_projection → no external edit → STOP
4. If disk_content != last_projection:
     old_blocks = parse(last_projection)    ← what Loro thinks the file should be
     new_blocks = parse(disk_content)       ← what's actually on disk
     diff = compute_diff(old_blocks, new_blocks)
     apply diff to Loro as CRDT operations  ← MERGE, not overwrite
5. Re-project from merged Loro state
6. Write merged projection to disk
```

This is correct because:
- If Loro is newer (P2P sync while offline): `last_projection` reflects Loro's state.
  The diff shows what the user changed in the org file vs what Loro expects. Both are preserved.
- If the org file is newer (user edited while app was off): the diff captures the user's
  edits as CRDT operations applied on top of Loro's state.
- If both changed: CRDT merge combines them. The re-projection reflects the merged state.

**This is the Automerge approach**: detect what changed vs last-known state, express changes
as CRDT operations, let the CRDT merge handle conflicts.

### Why `known_state` Is the Wrong Abstraction

The current `known_state` tries to answer: "what does Loro think the org file contains?"
But it's populated via async EventBus events and can be stale.

`last_projection` answers the same question but is **always correct by construction**:
it's literally the content we last wrote (or confirmed) on disk. There's no async
subscription, no race condition, no stale state.

### Comparison

| Aspect | Current (Debounce) | Proposed (Projection) |
|--------|-------------------|----------------------|
| Echo suppression | WriteTracker + 2s window | `disk == last_projection` (permanent) |
| Diff base | `known_state` (async, can be stale) | `last_projection` (synchronous, always correct) |
| Partial render | Possible if debounce expires early | Impossible (render is atomic per file) |
| Startup ordering | Must carefully sequence OrgAdapter vs OrgFileWriter | Just render from Loro, then detect diffs |
| Write batching | Conflated with correctness | Separate concern (perf optimization only) |
| Testability | Hard (timing-dependent) | Easy (pure functions + state machine) |
| Components | 2 (OrgAdapter + OrgFileWriter) | 1 (OrgSyncController) |

## 9. Concrete Implementation Sketch

### The OrgSyncController

```rust
pub struct OrgSyncController {
    /// The content we last wrote to (or confirmed on) disk, per file.
    /// This is the diff base for detecting external edits.
    last_projection: HashMap<PathBuf, String>,

    /// Loro backend for reading/writing blocks
    loro: Arc<LoroBackend>,

    /// Write coalescing: tracks which files need re-rendering.
    /// Purely a performance optimization — not required for correctness.
    dirty_files: HashSet<PathBuf>,
}

impl OrgSyncController {
    /// Called at startup: render Loro state to establish the diff base.
    async fn initialize(&mut self) {
        for (path, doc) in self.loro.iter_documents() {
            let blocks = doc.get_all_blocks();
            let rendered = OrgRenderer::render(&blocks);
            self.last_projection.insert(path.clone(), rendered);
        }
    }

    /// Called when FileWatcher detects a change.
    /// No timing dependencies. No WriteTracker. No debounce.
    async fn on_file_changed(&mut self, path: &Path) {
        let disk_content = fs::read_to_string(path).unwrap();
        let last = self.last_projection.get(path).cloned().unwrap_or_default();

        if disk_content == last {
            return; // Our own write, or genuinely unchanged. Done.
        }

        // Diff against what WE last wrote, not against Loro's current state.
        let old_blocks = parse_org(&last);
        let new_blocks = parse_org(&disk_content);
        let ops = diff_blocks(&old_blocks, &new_blocks);

        // Apply external changes as CRDT operations (merge, not overwrite)
        for op in ops {
            self.loro.apply(op).await;
        }

        // Re-project: Loro may have merged with concurrent changes
        let blocks = self.loro.get_all_blocks(path).await;
        let rendered = OrgRenderer::render(&blocks);

        if rendered != disk_content {
            // Loro merged something: write back the merged result
            fs::write(path, &rendered).unwrap();
        }

        self.last_projection.insert(path.to_owned(), rendered);
    }

    /// Called when Loro changes (UI mutation, P2P sync, etc.)
    async fn on_loro_changed(&mut self, affected_file: &Path) {
        let blocks = self.loro.get_all_blocks(affected_file).await;
        let rendered = OrgRenderer::render(&blocks);
        let last = self.last_projection.get(affected_file).cloned().unwrap_or_default();

        if rendered == last {
            return; // No net change to this file.
        }

        fs::write(affected_file, &rendered).unwrap();
        self.last_projection.insert(affected_file.to_owned(), rendered);
    }
}
```

### What Disappears

With this architecture, the following components are **no longer needed**:

| Component | Why it's eliminated |
|-----------|-------------------|
| `WriteTracker` | Replaced by `disk_content == last_projection` |
| `known_state` | Replaced by `parse(last_projection)` |
| `EventBus subscription in OrgAdapter` | No async state to maintain |
| `OrgFileWriter` (as separate component) | Merged into `OrgSyncController.on_loro_changed()` |
| `OrgAdapter` (as separate component) | Merged into `OrgSyncController.on_file_changed()` |
| Debounce loop | Replaced by optional write-coalescing (perf only) |
| Origin filtering for echo suppression | Content comparison is sufficient |

### What Remains (unchanged)

- `LoroBlockOperations` + CRDT merge (core strength of the system)
- `LoroEventAdapter → TursoEventBus → CacheEventSubscriber` (Loro → Turso cache → UI)
- `IrihSyncAdapter` (P2P, feeds into Loro which triggers `on_loro_changed`)
- `OrgRenderer` and `parse_org_file` (rendering/parsing, but must satisfy roundtrip property)
- Write coalescing as a performance optimization (optional, separate from correctness)

## 10. PBTs for the New Architecture

The projection-based architecture is far easier to test because it's a pure state machine:

### PBT-1: Roundtrip (MOST IMPORTANT, tests I5 + I6)

```
Property: parse(render(blocks)) ≡ blocks
```

This is the foundational invariant. If this holds, the entire sync loop is correct
by construction. If it doesn't, no amount of debouncing or timing will save you.

### PBT-2: OrgSyncController as State Machine

```
State: (Loro blocks, last_projection per file, org files on disk)
Transitions:
  - ExternalEdit(file, new_content): modify org file
  - LoroMutation(block_id, new_content): modify block in Loro
  - FileChanged(file): trigger on_file_changed
  - LoroChanged(file): trigger on_loro_changed

Invariant after each transition:
  - last_projection[file] == disk_content(file)  (always in sync)
  - parse(last_projection[file]) ≡ Loro.blocks(file)  (Loro and projection converge)
```

This is a straightforward stateful PBT with no timing dependencies. It can run thousands
of cases in seconds.

### PBT-3: Concurrent Convergence

```
Generate: interleaved ExternalEdit + LoroMutation sequences
Assert: after processing all events, system converges regardless of order
```

### PBT-4: Startup with Divergent States

```
Setup: Loro has state A, org file has state B (A ≠ B)
Run: initialize() then on_file_changed()
Assert: Loro and org file converge to merge(A, B)
```

## 11. Invariant Map (Revised)

| Invariant | How Enforced | PBT |
|-----------|-------------|-----|
| I1: Convergence | Structurally: single controller, immediate re-projection | PBT-2, PBT-3 |
| I2: No stale overwrite | Diff against last_projection (not stale known_state) | PBT-4 |
| I3: Loop termination | `disk == last_projection` → STOP (no timing) | PBT-2 |
| I4: Idempotency | Diff(same, same) = empty → no ops | PBT-2 |
| I5: Deterministic render | Roundtrip property | PBT-1 |
| I6: Roundtrip preservation | Roundtrip property | PBT-1 |
| I7: Write count bound | Each mutation → at most 1 write per affected file | PBT-2 |
| I8: Startup correctness | initialize() sets last_projection from Loro before file scan | PBT-4 |

## 12. Migration Path

### Phase 1: Roundtrip PBT (low risk, high value)
Add PBT-1 (`parse(render(B)) ≡ B`). Fix any normalization bugs found. This is
prerequisite for everything else and helps the current architecture too.

### Phase 2: Introduce `last_projection` alongside existing system
Add `last_projection: HashMap<PathBuf, String>` to OrgAdapter/OrgFileWriter.
Use it for assertions only (compare with existing known_state behavior, log differences).
This validates the approach without changing behavior.

### Phase 3: Replace `known_state` with `last_projection`
Change `on_file_changed()` to diff against `last_projection` instead of `known_state`.
Remove the async EventBus subscription in OrgAdapter. Remove `WriteTracker`.
Run existing E2E PBT to verify identical behavior.

### Phase 4: Merge OrgAdapter + OrgFileWriter
Create `OrgSyncController`. Remove the debounce loop (replace with optional write
coalescing). Remove origin filtering for echo suppression. Add PBT-2 through PBT-4.

### Phase 5: Add `blocks_differ()` completeness
Add `source_language` and `source_name` to comparison. Add PBT-U2.
