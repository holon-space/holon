# Deep review: consolidator / sync pipeline (Loro → Turso)

Reviewer: Fable agent, 2026-07-06. Read-only pass over `holon-loro`, `holon-filesystem`,
`holon-turso`, `holon/src/core`, `holon-app`, `holon-api`, plus
`docs/Architecture/{Sync,Replication,Storage}.md`.

---

## 1. Pipeline map (what runs per commit)

Every Loro commit (each keystroke batch, each peer import) triggers the FULL chain below.
Nothing except the Loro snapshot-save is per-snapshot; the whole projection is per-commit.

```
Loro commit
  → doc.subscribe_root callback fires wake            loro_sync_controller.rs:151-153
  → run_loop: wake.notified → on_loro_changed         loro_sync_controller.rs:188-201
  → LoroProjection::project()  [project_lock]         loro_sync_controller.rs:349-509
      1. current_frontiers()                          :353  (oplog frontiers, cheap)
      2. FULL-DOC snapshot                             :360-364
         snapshot_blocks_from_doc_settled              loro_backend.rs:837-919
           tree.get_nodes(false) + get_meta + read_block_from_tree per node  → O(N)
           effective_sibling_sort_keys per group       loro_backend.rs:930-960  → O(k²)/group
      3. base read (Arc, cheap) or cold-boot SQL seed  :379-385
         TursoSinkReader::read_blocks (full block_raw
         scan + per-row junction subqueries)           turso_sink_reader.rs:37-...
      4. diff_snapshots_to_ops(before, after)          :388, impl :607-675  → O(N)
         blocks_differ per common id                   :917-927 (clones properties ×2, block.rs:575)
      5. delete-pass gate (armed / settled)            :404-416
      6. BlockConsolidator::apply(ops)                 :462 → consolidator.rs:71-108
           ChangeSet::from_ops (typed-intent shadow)   change_set.rs:155-173
           agrees_with_ops → agree/divergence counters change_set.rs:211-224
           execute_batch_with_origin                   sql_operation_provider.rs:1201-1257
             prepare_create / prepare_update / prepare_delete per op
               prepare_update: 2 extra SELECTs/block   :593-604 (props merge), :671-681 (diff guard)
               prepare_delete: cascade discovery query
             ONE transaction for the whole batch       :1245-1248
      7. Turso actor commits → IVM maintains matviews (chained) → CDC broadcast
           DbHandle.cdc_broadcast + cdc_seq            turso.rs:232-236
           MatviewManager demux → per-view streams     matview_manager.rs:274-360
           → LiveData / widget resample (its own known 83% cliff, out of scope here)
      8. put_base(after) → SyncBaseStore::persist:
         serde_json of the ENTIRE base map + fs::write sync_base_store.rs:135-166, :184-190
      9. watermark advance + frontiers sidecar write   :506, :533-542
```

Key structural fact: since the Phase-3 base-diff flip, the `last_synced` frontiers
watermark is **not used to bound the diff at all** — `project()` uses it only to hex-encode
`provenance.base_ref` (:449-457). The `subscribe_root` event payload (which carries the
incremental delta) is **discarded** (`move |_event|`, :151). The projection is a
full-state reconcile on every wake.

## 2. The 1.84s dominator — yes, O(full doc) per commit

Confirms the latency-instrumentation finding (0.221 ms/block/commit → 1–2 s at vault
scale). Four O(N) costs stack per commit, only one of which is the actual SQL write:

1. **Full-tree snapshot** — `snapshot_blocks_from_doc_settled` (loro_backend.rs:837)
   walks every live node, reads meta, materializes a full `Block` per node. Pure O(N)
   regardless of what changed. `effective_sibling_sort_keys` (:930-960) is additionally
   O(k²) per sibling group (per-sibling `filter` over all sibling fis, twice for tied runs)
   — quadratic in wide sibling groups (journals page with hundreds of children).
2. **Full diff** — `diff_snapshots_to_ops` iterates all of `after` + `before`;
   `blocks_differ` (:917-927) calls `properties_map()` which **clones the whole property
   HashMap** (block.rs:575-577) twice per block per pass. `HashMap: PartialEq` on
   `&a.block.properties == &b.block.properties` would be allocation-free.
3. **Base persist** — on any changed pass, `put_base` → `persist()`
   (sync_base_store.rs:135-166) re-serializes the **entire document-sized base map to JSON
   and rewrites the whole sidecar file with blocking `std::fs::write` on the async runtime**.
   At vault scale this is megabytes of serde_json per keystroke-commit. This is very likely
   the single biggest avoidable line item after the snapshot itself.
4. **Redundant re-query in prepare_update** — per changed block, two SELECT round-trips
   through the DB actor (sql_operation_provider.rs:593-604 properties read-merge,
   :671-681 per-column diff guard). Both re-read state the projection *already has as the
   base* — by construction the base IS the last-projected sink state, so the "old row" is
   known before the SQL layer is entered. During bulk passes (boot, org re-scan) this
   multiplies actor round-trips by ~3× per updated row.

Only step 6's transaction and step 7's IVM/CDC are O(changed); everything else is O(doc).

### Algorithmic fix (proposed, in order of leverage)

- **Dirty-set tracking (the real fix).** Stop discarding the subscription payload: the
  `subscribe_root` event (or `doc.diff(last_synced, current)` — Loro exposes exactly this
  between two frontiers) yields the touched containers/TreeIDs. Per pass:
  collect dirty stable-ids + their (old and new) parent sibling-groups; run
  `read_block_from_tree` + `effective_sibling_sort_keys` only for those; diff only those
  ids against the base map; leave the rest of the base untouched. Everything needed
  already exists — `diff_snapshots_to_ops` works unchanged on the restricted key set,
  and the base map can be updated in place per dirty id instead of wholesale replaced.
  Fall back to the current full reconcile when the event is a bulk import / first boot
  (and optionally as a low-frequency background sweep as a self-check oracle — keeps the
  convergent-feed guarantee testable).
- **Debounce + delta the base persist.** Even before dirty-sets: persist the base at most
  every T seconds / on shutdown / on frontier idle, via tmp-file + rename (atomicity —
  today a crash mid-`fs::write` leaves a corrupt sidecar). With dirty-sets, switch the
  sidecar to an append-friendly or sharded form, or drop the JSON base entirely and store
  `(loro snapshot bytes at frontier F)` + F — the base is by definition a Loro state, so
  a `fork_at(F)`-reconstructed snapshot can replace the JSON copy.
- **Kill the per-update SELECTs.** Pass the base's old row alongside the update op (or an
  explicit "old properties" param); `prepare_update` then merges/diff-guards in memory.
  The diff guard becomes an assertion, not a query.
- **Micro:** compare properties without cloning; memoize `properties_map`; O(k) tie-run
  detection in `effective_sibling_sort_keys` (single pass with a run counter per fi).

## 3. Correctness

### 3a. Epoch guard (consolidator_epoch.rs)

Scope check first: invariant 10 is an **identity epoch** guard ("same consolidator as the
one that wrote this durable state"), not a mutual-exclusion lock. For that stated purpose
it is sound and properly fail-loud (mismatch = hard error, corrupt marker = mismatch not
parse-error, durable-state-without-marker-dir = bail :62-68). Gaps:

- **TOCTOU on first boot** (:71-79): `marker_path.exists()` → `write_marker` is not
  atomic. Two processes first-booting the same data dir concurrently with *different*
  configured consolidators both see "no marker", both write, both proceed — exactly the
  mixed-keyspace corruption the module exists to prevent. Fix: `OpenOptions::create_new`
  (O_EXCL); on `AlreadyExists`, fall through to the read-and-compare path.
- **No single-writer enforcement at all**: two processes with the SAME consolidator id
  both pass the guard and run two independent `LoroProjection`s (separate `project_lock`s,
  separate base sidecars) against one `block_raw`. Their base stores then disagree with
  the sink and each other → ping-pong rewrites. Nothing else in the tree takes a data-dir
  lock. If multi-process is out-of-contract, enforce it: a `flock`ed lockfile next to the
  marker, fail loud on contention.
- **Crash window in migrate** (:86-97): `wipe_durable_state` then `write_marker` — a crash
  between them leaves wiped state + stale marker; next boot without MIGRATE=1 hard-errors.
  Fail-loud, so acceptable, but the error message will mislead (state is already gone).
  Writing the new marker *before* the wipe would leave re-runnable state instead.
- **Sidecar writes are non-atomic everywhere** (marker :125, frontiers sidecar
  loro_sync_controller.rs:539, base store sync_base_store.rs:161): plain `fs::write`,
  torn on crash. Frontiers-sidecar corruption degrades benignly (warn + empty watermark,
  :573-581 — and the base-diff makes the watermark near-vestigial anyway), but use
  tmp+rename as a matter of course.

Within one process the design is solid: single `BlockConsolidator` behind one
`LoroProjection`, `project_lock` serializes run-loop vs org-flush, watermark and base
advance only under that lock, and a failed sink write (`?` at :462) leaves both
unadvanced so the next wake retries. No lost-write inside the process.

### 3b. `ChangeSet::from_ops` divergence — root-caused

The 11→10 "drops 1 of 2 update ops" is **not an op-merge bug in from_ops itself**; it is
an update op that decodes to *zero* typed ops, plus a real data-loss bug underneath:

- `decode_update` (change_set.rs:271-310) pushes `Relocate` and per-field `SetField`,
  skipping `UPDATE_BOOKKEEPING_FIELDS` (`id`, `updated_at`, …). An update whose params
  are only `{id, updated_at}` therefore contributes **nothing**, and
  `reencode_op_names` (:179-195) counts updates by distinct SetField/Relocate ids —
  so source says `update: 2`, re-encode says `update: 1`. One op "dropped".
- **When does the projection emit an `{id, updated_at}`-only update?** When
  `blocks_differ` is true *solely because a property was REMOVED*:
  `block_diff_params` (loro_sync_controller.rs:875-887) detects
  `old.properties_map() != new.properties_map()` but then only iterates
  `new.properties` — a key present in `old` and absent in `new` emits **nothing**.
  (Secondary same-shape case: the only differing property is an edge column, skipped at
  :882-884 — but edges are also compared/emitted separately, so removal-only is the case.)
- **This is a real prod data bug, not just a shadow-counter artifact.** The SQL layer's
  property merge removes a key only on the explicit `Value::Null` sentinel
  (sql_operation_provider.rs:632-641), which the org-header path emits but the Loro
  projection never does. So: delete a property in Loro → SQL `properties` JSON keeps it
  **forever** (the base advances to `after`, so it is never re-diffed — one-shot, silent).
  This also matches the flavor of the memory-flagged `org_ingest_drops_block_marks` class.
- **Fix (small):** in `block_diff_params`, when the property maps differ, additionally
  emit `k → Value::Null` for every key in `old.properties` missing from
  `new.properties` (mirroring the marks/source_language clear handling directly above
  it, :846-871 — this exact bug shape was already fixed for those scalar fields in the
  2026-07-04 `consolidator_source_language_clear_fix`; properties got missed).
  That simultaneously kills the divergence: the update then decodes to a SetField(Null).
  Re-check the set-aside seed after.
- `reencode_op_names`'s id-set dedup (:182-190) is a second latent divergence source if a
  batch ever carries two update ops for one id (diff never does today) — fine to leave,
  but worth a comment.

### 3c. Silent-failure audit of the write path

The core sink write is clean: `consolidator.apply` propagates (`consolidator.rs:103-106`),
`project()?` propagates, batch runs in one transaction with error propagation
(sql_operation_provider.rs:1245-1248). No `.ok()` on the SQL write. Divergences are
counted-not-aborted by documented design. But around the edges:

- **Failed projection is retried only on the next doc change** — `run_loop`
  (loro_sync_controller.rs:188-201) consumes the wake permit, logs, increments
  `error_count`, and waits for a NEW wake. If the *last* commit's projection fails
  (transient Turso lock, disk full), the sink stays stale **indefinitely** while Loro
  holds newer data — a real lost-write-window for readers, silent beyond one log line.
  Fix: on error, `wake.notify_one()` after a backoff (cap retries, then loud).
- **CDC broadcast lag drops changes without resync** — both the `DbHandle` stream
  (turso.rs:632) and the MatviewManager demux (matview_manager.rs:354-358) handle
  `RecvError::Lagged(n)` with a warn and continue. Subscribers permanently miss those row
  changes; nothing re-snapshots the view. This is a concrete mechanism for the memory-
  flagged transient stale-sidebar symptoms. Fix: on Lagged, push a synthetic
  "resync" marker into the per-view stream so consumers re-pull initial rows.
- **`SyncBaseStore::persist` warns-and-returns** on serialize/mkdir/write failure
  (sync_base_store.rs:135-166). Disclosed (warn), and next boot cold-seeds from SQL, so
  tolerable — but a *persistent* failure means silent cold-boot degradation every launch;
  a repeated-failure counter that escalates to error would honor fail-loud better.
- Minor: `block_to_params`'s `entry().or_insert_with` (:796) lets a stray block property
  named like a first-class column (`content`, `parent_id`) be silently shadowed — an
  assertion would be cheaper than the debugging session when it happens.

## 4. Prioritized fixes

| P | What | Where |
|---|------|-------|
| P0 | Emit `Value::Null` for removed properties in the update diff (data loss + the from_ops divergence; re-run set-aside seed) | loro_sync_controller.rs:875-887 |
| P1 | Dirty-set incremental projection: consume `subscribe_root` delta / `doc.diff(frontiers)`, snapshot+diff only touched ids & sibling groups; keep full reconcile as fallback/oracle | loro_sync_controller.rs:151,349-509; loro_backend.rs:837 |
| P1 | Stop rewriting the whole JSON base per commit: debounce + tmp/rename; longer term store frontier-pinned Loro state instead of a JSON copy | sync_base_store.rs:135-166,184-190 |
| P2 | Re-arm wake with backoff on projection failure (stale-sink window) | loro_sync_controller.rs:188-201 |
| P2 | CDC `Lagged` → per-view resync signal instead of warn-and-drop | turso.rs:632; matview_manager.rs:354-358 |
| P2 | Epoch marker: O_EXCL first-boot write; add a flock'd data-dir lock (single-writer) | consolidator_epoch.rs:71-79,112-127 |
| P3 | Drop prepare_update's two per-block SELECTs — the base already holds the old row | sql_operation_provider.rs:593-604,671-681 |
| P3 | Allocation-free property compare; O(k) tie-run sort keys; atomic sidecar writes | loro_sync_controller.rs:925; block.rs:575; loro_backend.rs:930-960 |

Cross-check note: P1 dirty-set work should land with a PBT hook — the composed keystone
already drives this path; a metamorphic invariant "incremental pass ≡ full reconcile"
falls out for free by keeping the old full path as the oracle.
