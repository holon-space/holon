# Never-seeded block adoption (D175.a)

*Plan. Nothing here is implemented yet. Line references were read at
`3c4442b8`; the implementation base is **post-F1a main** (lane `2f62ae90`
weaving onto it) — see §4.0 for what F1a carries. Verify the line numbers with
the §5 staleness greps before editing.*

**The trait comes first, and adoption is expressed through it.** Martin ruled
on `docs/Plans/existence-authority-alternatives-2026-09-22.md` §6 (2026-09-22):
Approach A with C1, and — ruling 5 + Q6 — **build the `Consolidator` /
`Replica` / `TextFormat` abstraction now**, with Loro and the keystone model as
the two implementations and `GitConsolidator<F>` declared, not implemented. So
this plan's increments start with the seam (Inc A), then the existence rule as
an assertion through it (Inc B), and only then the veto deletions.

The other rulings that change this plan:

| # | Ruling | Effect here |
|---|---|---|
| 1 | Delete-amplitude: **Honour** by default, disclosed and revertible; a Quarantine strategy behind one policy point. | Out of scope; §5 names the seam it plugs into. |
| 2 | File resurrection: resurrect when the **file base** proves it (`X ∉ base ∧ X ∈ file`); a delete wins when `X ∈ base`. | Inc B's resurrection rule. |
| 3 | Turso as a registered GC base — ratified. | Makes `ever_seen`'s tombstones durable (see Inc A). |
| 4 | Single adopter = home-file holder; no adoption under mount/pairing — ratified. | §2c is settled; the guard is Inc C3. |
| R9/R10 | **No authored field is SQL-only** (measured). | Adoption from the org file loses nothing — this removes the last argument for a row-sourced adoption path. |

**D178.a** — the background pass is measure-first: §3's measurement decides
whether C6 is written at all.
**D179.d** — Text+VCS is the second consolidator; Turso is not one yet, so
`ever_seen` has a Loro implementation and a keystone-model implementation, and
no Turso implementation is written.

Martin's ruling D175.a: **Loro is the authority for block existence; SQL is a
derived index.** Today the org scanner asks Loro "does this block exist", is
told "no", and then lets SQL's "yes" veto that answer. Rows born before the
Loro store therefore never get a Loro node, and the D172.a Loro-fed read model
cannot show them. The fix is to delete the veto: absence in Loro means *create
the node*, with the block ID preserved, through the same path a new block
takes.

---

## 0. First principles

0. **No code above the seam names a store.** The existence rule, adoption, the
   single-adopter rule and the duplicate-ID error are stated once against
   `Consolidator`; Loro is one implementation, the keystone model is another.
   This is ruling 5, and it is why Inc A precedes every behaviour change.
1. **Existence has one authority, judged against history — not current state.**
   An entity exists iff the consolidator says `Live`. A `block_raw` row whose
   id answers `Never` is an index entry for an entity that was never born, and
   adoption is that birth. A row whose id answers `Deleted(_)` is the opposite:
   a delete that has not finished projecting, and adopting it would resurrect
   it. Current-state absence cannot tell those apart — that is study §1.3's
   theorem, and C1 is the fix.
2. **Adoption is never a startup barrier.** Martin's constraint: it runs while
   normal start proceeds, exactly as if the blocks had arrived through an
   org-file change. The file-change path is the model, not a migration gate.
   A block becomes visible when its node exists, one at a time, the same way a
   file-changed block does.
3. **Identity is preserved, never re-minted.** Invariant 13: the block ID was
   minted once, when the block first entered the system. Adoption resolves
   before minting; it supplies the existing ID.
4. **A degraded window is disclosed or it is not allowed.** Invariant 14 ranks
   "silently degrades to look fine" last.

### The non-blocking mechanism to reuse (already exists)

| Seam | file:line |
|---|---|
| The whole scan runs in a **detached task** | `crates/holon-orgmode/src/di.rs:841-854` — `shutdown.spawn("file-sync-controller", run_file_sync_controller(...))` |
| The scan loop itself | `crates/holon-orgmode/src/di.rs:1174-1193` — `begin_initial_scan()` then `on_file_changed(&file_path)` per file |
| Readiness is a **signal, not a barrier** | `crates/holon-orgmode/src/di.rs:1278` — `sender.signal_ready()`; the factory already returned (`di.rs:1119-1120`: watcher arming is deliberately deferred *past* `signal_ready` so the factory returns immediately) |
| End-of-scan convergence point | `crates/holon-orgmode/src/di.rs:1196` → `FileSyncController::finish_initial_scan(30_000)`, `crates/holon-filesystem/src/file_sync_controller.rs:1139` |
| The **precedent for a post-scan unconditional sweep** | `crates/holon-orgmode/src/di.rs:1210` → `heal_title_less_doc_roots()`, `crates/holon-filesystem/src/file_sync_controller.rs:1542` |
| Boot seed phase ends | `crates/holon-orgmode/src/di.rs:1220` — `controller.finish_boot_seeding()` |
| Quiescence signal | `OrgSyncIdleSignal`, `crates/holon-orgmode/src/di.rs:134`; `wait_quiescent` / `mark_progress` / `processed_change_seq` (`di.rs:202-209`) |
| The **runtime** file-change loop — the model to imitate | `crates/holon-orgmode/src/di.rs:1351-1372` — a biased `tokio::select!` off the render path calling `on_file_changed(&file_path).await` per event; exits on `shutdown.cancelled()` (`:1363`) |
| Scan-in-progress flag a job can poll | `FileSyncController::in_initial_scan()`, `crates/holon-filesystem/src/file_sync_controller.rs:1126` (false after `finish_initial_scan`) |

`heal_title_less_doc_roots` is the exact shape this plan's background pass
needs, and its doc comment already states the reason: *"the ingest
byte-identity fast-path skips unchanged degraded files, so their heal cannot
live in ingest"* (`file_sync_controller.rs:1530-1537`). Same argument, same
slot.

### What the UI shows for a not-yet-adopted block

With the D172.a Loro-fed read model: **absent**. Identical to a file-changed
block between the file write and its scan. The block appears the moment its
node exists.

- For **(a)**, the in-scan adoption: **no disclosure needed.** The window is
  the initial-scan window every boot already has, and the vault is already
  drawing incrementally through it.
- For **(b)**, the background pass over files the scan never opens: **one
  disclosure**, because that window extends past `signal_ready` into a UI the
  user believes is fully loaded. One new `ConditionKind` (see §2b), `Severity::Info`,
  cleared by a `ClearingEvent` when the queue drains. `condition_bus.rs`
  already forbids `Elapsed` on anything but `Info`, so Info is the correct
  severity for a progress condition that ends on its own.

---

## 1. Mechanism (as the code stands today)

### 1.1 The new-block path in `on_file_changed`

`crates/holon-filesystem/src/file_sync_controller.rs`:

```
creates pass                 :4160-4249   walks new_blocks_vec (DFS document order)
  → PendingCreate            :4201/:4240
  → flush_pending_creates    :428-491
     → BlockOrdering::create_in_tree_batch
```

`crates/holon-core/src/block_ordering.rs`:

```
create_in_tree_batch(&[BlockCreateRequest])  :182
create_in_tree(parent, after, new_id, content, props, edges) -> Result<bool>  :161-171
```

`crates/holon-loro/src/block_cell_registry.rs`:

```
create_entities(requests)                    :705-771
  → warm_stable_id_cache (one walk per chunk) :713
  → create_block_with_properties(resolved_parent, content, Some(new_id), props, edges)  :649-658
```

### 1.2 Does the create path accept a caller-supplied block ID, or mint one?

**It accepts a caller-supplied ID. Nothing on this path mints.**

- `BlockCreateRequest.id: EntityUri` — `block_ordering.rs:28`
- `create_in_tree(..., new_id: &EntityUri, ...)` — `block_ordering.rs:161-169`
- the registry hands it straight to the backend as `Some(new_id.clone())` —
  `block_cell_registry.rs:653`

The **named function** is `EntityCellRegistry::create_entity` /
`create_entities` (`block_cell_registry.rs:520`, `:705`), reached through
`BlockOrdering::create_in_tree` / `create_in_tree_batch`. It is already
idempotent on a supplied ID: `block_cell_registry.rs:550-554` skips the create
when the node exists, *"`create_block` would mint a duplicate node for the same
stable id"*.

The **Loro `TreeID` is minted by Loro**, not by us, and the stable block ID is
carried in the node's meta map: `set_stable_id` (`loro_share_backend.rs:1816`),
looked up by `find_tree_id_by_stable_id` (`:1600-1610`). This is the single
fact that makes §2c (peers) a real problem.

### 1.3 Adoption already exists, and is already named

`file_sync_controller.rs:4180-4206` — `needs_reseed`:

```
old_blocks.contains_key(&block.id)
  && block.id != new_parse.document.id
  && consolidator == Upstream
  && ordering.in_tree(&block.id).await? == Some(false)
```

→ `PendingCreate { kind: PendingCreateKind::Reseed }`, handled at
`file_sync_controller.rs:466-477` (`"re-seeded pre-Loro vault block into the
Loro tree"`).

The trait documents the contract: `Ok(Some(false))` is *"the pre-Loro-vault
upgrade signal … The org-scan reconciler uses this to re-seed such blocks via
`create_in_tree`"* — `block_ordering.rs:200-210`.

So the machine exists and takes the ID. **What is missing is not a mechanism;
it is the removal of the vetoes that let the machine be skipped.**

### 1.4 The veto sites

| # | Site | Behaviour |
|---|---|---|
| **V1** | `file_sync_controller.rs:4655-4681` | Place loop. A block absent from `live_children` and not in `created_ids` → `warn` + `continue`. `ALLOW(fallback)`, rationale: *"bailing here aborted the whole initial scan and the app never started"*. |
| **V2** | `block_cell_registry.rs:788-812` | `live_children`: parent with no tree node → `Ok(None)` (SQL owns the order). `ALLOW(fallback)`. **This is what makes V1 fire** — `None` collapses to an empty `siblings` slice at `file_sync_controller.rs:4651`. |

Same family, adjacent, in scope to review but not named by D175.a:

| Site | Behaviour |
|---|---|
| `block_cell_registry.rs:540-549` | `create_entity` after-anchor guard → `Ok(false)` when the *anchor* has no node. Not on the reseed path (reseed passes `after_id: None`, `block_ordering.rs:189`). |
| `block_cell_registry.rs:502-507` | `create_entity_sync` parent guard → `Ok(false)`. |
| `block_cell_registry.rs:916-922` | `write_field("content")` → falls through to the SQL write. |
| `file_sync_controller.rs:478-487` | The **decline** branch: `create_in_tree` returned `false` for a Reseed → `"re-seed declined by the tree backing — order stays SQL-owned"`. This is the residue that reaches V1. |

Note the asymmetry that makes deleting V2 tractable: on the **async** create
path an absent parent is *not* fatal — `resolve_parent_or_placeholder`
(`block_cell_registry.rs:648`, `:745`) stands one up. Only the `_sync` variant
and `live_children` refuse.

### 1.5 How `want_after` is derived from file order

- `predecessors: HashMap<EntityUri, Option<EntityUri>>` built at
  `file_sync_controller.rs:4120-4134`, walking `new_blocks_vec` (DFS document
  order) and tracking `last_block_per_parent` (`:4096`). `None` = first child.
- Consumed at `file_sync_controller.rs:4648-4649` as `want_after`, then
  `self.ordering.place(&new_block.id, parent, want_after)` (`:4683-4686`).
- In the no-Loro arm the same document order is realised totally instead, via
  `place_all` per parent (`:4700-4730`).

So file order *is* the ordering intent, already in the right shape for an
adopted block: an adopted block is placed after its file predecessor exactly
like a fresh one.

### 1.6 Where boot ends

Sequence inside the one detached task (`di.rs:1140-1237`):

```
begin_initial_scan()                 di.rs:1174
  per-file on_file_changed           di.rs:1177
finish_initial_scan(30_000)          di.rs:1196   ← the quiescence point
materialize_missing_page_files()     di.rs:1199
heal_title_less_doc_roots()          di.rs:1210   ← the unconditional post-scan sweep slot
finish_boot_seeding()                di.rs:1220
...
signal_ready()                       di.rs:1278
arm() + watch loop                   (after :1278)
```

---

## 2. Design

### (a) Delete the veto in `on_file_changed` — a scan adopts as it goes

Two edits, in this order:

1. **V2 → adopt instead of abstain.** `live_children`
   (`block_cell_registry.rs:788-812`): when the parent has no node, do not
   return `None`. The parent is reachable — `resolve_parent_or_placeholder`
   already stands up an absent ancestor chain on the async path
   (`block_cell_registry.rs:648`). Return the real children list. A parent that
   still cannot be resolved is an `Err`, not a `None`.
2. **V1 → delete the `continue`.** `file_sync_controller.rs:4655-4681`: with
   (1) in place, a pre-existing block reaching this branch means adoption
   failed upstream, which is a bug. Collapse the two arms into the existing
   `bail!` at `:4657`.

Why this is enough *for a file the scan opens*: every block in
`new_blocks_vec` takes one of two branches in the creates pass — `needs_reseed`
(`:4186`, in `old_blocks`) or `!old_blocks.contains_key` (`:4207`, fresh) — and
**both** end at `create_in_tree_batch`. Document order guarantees parents
precede children (`:4182-4184`), so parent-first is already satisfied. The only
way a block reaches V1 today is the decline at `:478-487`, whose sole cause is
the missing-parent guard that (1) removes.

Consequence to accept deliberately: with V1 gone, a genuinely broken vault
aborts its *file*, not the app — `run_file_sync_controller` collects per-file
failures and falls through to `arm()` (`di.rs:1239-1252`, with an explicit "do
not reinstate the early return" guard). The 2026 rationale for the fallback
("the app never started") no longer holds; that regression was fixed at the
driver level.

### (b) Rows whose file the scan does not open

**Does the initial scan touch every file? It enumerates every file, but it does
not ingest every file.**

- `scan_vault_files` (`di.rs:1141`) enumerates all org files.
- `on_file_changed` then takes the **cold-boot fast path** at
  `file_sync_controller.rs:3282`:
  `stored == &disk_hash && self.content_present_in_all_stores(root).await?`
  → `return Ok(IngestOutcome::Ingested)` at `:3341`, having parsed nothing.
- `content_present_in_all_stores` (`file_sync_controller.rs:1514-1523`) asks
  `in_tree` about **the document root only**. A file whose doc-root has a node
  but whose interior blocks do not passes this gate and is skipped.

And the freeze-in is mechanical: `projection_hash`
(`file_sync_controller.rs:1488-1498`) mixes the consolidator tag into the hash,
so the *first* Loro-enabled boot after a `[loro] enabled` flip re-ingests
everything (`:1481-1487`). That is precisely the boot which half-adopts under
V1/V2 — and then re-stamps the hash under the Loro tag (`:5034`, `:5100`).
From boot #2 onward the never-seeded rows sit behind the fast path forever.

**Therefore (a) alone does NOT suffice.**

Preferred fix, **(b1) — strengthen the gate, let the normal path do the work**:
widen `content_present_in_all_stores` from "the doc root has a node" to "every
block this file's row claims has a node". The file's block membership is
already persisted beside `file.content_hash` for the read-only case
(invariant 14; `persisted_read_only_blocks`, `file_sync_controller.rs:3312`),
and the same row is where a writable file's membership would live. The fast
path then refuses, the ordinary ingest runs, and (a) adopts. No new job, no new
code path, and it is *exactly* the file-change model Martin named. Cost: one
membership comparison per file per boot.

Fallback, **(b2) — a background pass**, needed only for rows whose home file no
longer exists on disk (nothing will ever rescan them):

- Runs in the same detached task, in the `heal_title_less_doc_roots` slot
  (`di.rs:1210`), **after** it, **before** `finish_boot_seeding()`. Never
  awaited by anything before `signal_ready` (`di.rs:1278`).
- Query `block_raw` for rows with no Loro node; adopt parent-first, ordered by
  `(depth, sort_key)`; chunk at `CREATE_CHUNK_BLOCKS` through the same
  `create_in_tree_batch`.
- Disclosed: one new `ConditionKind` — proposed
  `VaultBlocksBeingAdopted { remaining: usize }`, `Severity::Info`, cleared by
  a `ClearingEvent` on drain (`crates/holon-api/src/condition_bus.rs:30`,
  profile in `condition_profile.rs`).

Recommendation: land (a) + (b1). Hold (b2) until the §3 measurement says how
many rows have no live home file; if that number is 0, (b2) is unnecessary and
should not be written.

**Rejected alternatives**

- *A boot migration that adopts everything before the UI opens* — violates
  Martin's constraint directly; also makes a 16k-block vault's first Loro boot
  a blank window.
- *Keep the veto, teach the read model to union SQL-only rows* — enshrines two
  existence authorities, which is the bug D175.a rules against.
- *Delete the never-seeded rows and let the org files re-ingest them* — loses
  every SQL-only field the files do not carry, and invariant 10 already names
  wipe-and-reseed as an interim hack, not a design.
- *Adopt lazily, when a row is first read* — a read path that writes to the
  consolidator; and it never converges for rows nothing reads.

### (c) Peers: two adopters would fork the block

**The hazard, concretely.** The `TreeID` is `(PeerID, Counter)` and is minted by
Loro at create time; the stable block ID lives in the node's meta
(`set_stable_id`, `loro_share_backend.rs:1816`). Two peers that each adopt the
same never-seeded block ID mint **two distinct TreeIDs carrying one stable
ID**. Tree merge keeps both. The snapshot builder then silently last-write-wins
them — `blocks.insert(block.id.to_string(), ...)` at
`crates/holon-loro/src/loro_backend.rs:1414`, with the only signal an
env-gated debug warn (`[LORO_DUP] duplicate stable id …`, `:1404-1413`). One
node's content vanishes into an unreachable orphan, with no disclosure. That is
the failure mode invariant 14 ranks last.

**Ruled (ruling 4, 2026-09-22): a single adopter — the peer that holds the
block's home file — and no adoption at all while a share mount or pairing
exists.** No longer a proposal.

- *Which code decides.* The block→home membership the file-sync controller
  records at ingest and persists beside `file.content_hash` (invariant 14,
  `docs/Architecture/Model.md:196-202`), read through `WriteTierAuthority`
  (`crates/holon-core/src/write_tier_gate.rs`). "This peer holds the file" is
  the same question that authority already answers for read-only homes; the
  adoption guard is one more caller, not a new authority.
- *The mount/pairing half.* Own-device pairing already refuses to run while any
  per-subtree mount exists (Model.md:142-145, ADR 0033). Adoption takes the
  mirror rule: refuse while a mount or pairing is live, and let the next
  unpaired boot adopt.
- *Make the fork loud regardless.* Promote the `HOLON_LORO_DUP_DEBUG`-gated
  warn (`loro_backend.rs:1404`) to an unconditional `error!` plus an
  `inv-no-observed-errors`-visible signal. Whatever rule we pick, a duplicate
  stable ID must never be silent again. This is independently landable and
  should go first.

**Rejected alternatives**

- *Deterministic TreeID derived from the block ID* — not expressible: `TreeID`
  is `(PeerID, Counter)`, and a peer cannot author another peer's PeerID
  without forging op provenance.
- *Adopt only when no peer connection exists* — connection state is transient;
  an offline peer adopts, then connects, and the fork arrives anyway.
- *Adopt then dedupe on merge* — a merge-time dedupe must pick a loser and
  discard its ops; that is re-merging in a sink, which invariant 5 forbids.

---

## 3. Cost bound

**Count of never-seeded rows on Martin's vault: unknown.** It must be measured
before (b2) is written.

**Complexity.** Let `R` = never-seeded rows, `C` = `CREATE_CHUNK_BLOCKS`.

- Adoption: `R` create ops, issued as `⌈R/C⌉` `create_in_tree_batch` calls, one
  `warm_stable_id_cache` tree walk per chunk (`block_cell_registry.rs:713`) —
  i.e. `O(R + ⌈R/C⌉·nodes)`, *not* the `O(R·nodes)` the per-block path would
  cost (`block_ordering.rs:694-700` documents exactly this).
- Parent-first ordering for (b2): `O(R log R)` sorting by `(depth, sort_key)`.
- Write amplification: `R` Loro ops → `R` DiffEvents → `R` SQL row writes, once
  per vault lifetime (`in_tree` then answers `Some(true)` forever).
- For (b1) instead: `O(files)` extra membership comparisons per boot, forever —
  but each is a comparison against a row already loaded at
  `file_sync_controller.rs:1395-1415`.

**The measurement is a hard precondition of Inc 3** — the first increment that
adopts. It must be run before that increment lands, not before Inc 4: `R` is
what decides whether the adoption wave fits inside `finish_initial_scan`'s 30 s
convergence ceiling (`di.rs:1196`).

**The measurement to take** — read-only against Martin's live vault, described
here, not run. Every step below is a read; none writes:

1. `mcp__holon-live__holon_live__diff_loro_sql` — the two-way Loro↔SQL
   divergence report. Its "in SQL, not in Loro" side *is* the never-seeded set,
   per row. Caveat: `docs/Testing/bugfunnel/entries/2026-08-05-diff-loro-sql-wrong-both-directions.md`
   — confirm the direction labels before trusting the split.
2. Cross-check the totals independently, so a tool bug cannot hide the number:
   - `execute_raw_sql`: `SELECT count(*) FROM block_raw;`
   - `execute_raw_sql`: `SELECT depth, count(*) FROM block_raw GROUP BY depth ORDER BY depth;`
     (gives the parent-first cost profile and the adoption wave shape)
   - `inspect_loro_blocks` for the tree-side count.
   - `R ≈ SQL count − Loro count`.
3. Split `R` by home-file liveness, which decides whether (b2) is needed at all:
   join the never-seeded IDs against the `file` rows and count those whose home
   file is absent from `scan_vault_files`' enumeration. If that count is 0,
   (b1) covers everything and (b2) is not written.
4. Record the wall-clock of one adoption wave against
   `docs/Testing/KeystoneKnownReds.md`'s profile rule — **name the build
   profile**; the test profile runs ~11–12× slower than release, so any latency
   claim about the adoption wave needs one release run.

---

## 4. Red-first keystone

*Per `.claude/skills/holon-feature/SKILL.md`: red-for-the-right-reason before
implementation, green after, the red log in the PR.*

### 4.0 State after the F1a read-model lane lands

This plan's base is **post-F1a** (lane rev `2f62ae90`, weaving onto
`3c4442b8`). Reconnaissance run against `3c4442b8` alone does not see the
following, all of which F1a carries — implementation starts on the post-F1a
main, not on `3c4442b8`:

- the **per-row `loro_node=Never` diagnostic** —
  `ReadModelObservation::sql_only_diagnostics`,
  `crates/holon-integration-tests/src/pbt/invariants/bodies/view_model_matches_store.rs`.
  This is the red signal; the lane does not build it.
- the bugfunnel entry
  `docs/Testing/bugfunnel/entries/2026-09-22-unseeded-vault-blocks-are-sql-only-so-a-loro-fed-read-model-cannot-show-them.md`.
- a **default** `HOLON_HAND_AUTHORED_SKIP` value in `justfile:257` naming
  `reboot-orphans-the-previous-boots-watchers`, plus its row in
  `docs/Testing/KeystoneKnownReds.md`.

And `reboot-orphans-the-previous-boots-watchers`
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl:744`)
**is** the D175 witness. Its original defect — the previous boot's watcher
tasks reading a closed Turso actor
(`crates/holon-integration-tests/src/pbt/transitions/reboot.rs:83-92`, ledger
entry `2026-09-08-reboot-orphans-watcher-tasks.md`) — is fixed; the case is
reused. It reboots into an unseeded vault and, on the strict three-way
invariant, reds **3/10** with `loro_node=Never` rows.

### 4.1 Inc 1: un-skip the case and log the red

1. Delete the `reboot-orphans-the-previous-boots-watchers` default from
   `justfile:257` and its row from `docs/Testing/KeystoneKnownReds.md`.
2. Run `just hand-authored` and capture the red: **3/10, `loro_node=Never`
   rows** on the strict three-way invariant. That signature *is*
   red-for-the-right-reason — it names never-seeded rows directly. The log goes
   in the PR.

Oracles this case must be able to fail (assertions, not prose):

- every `block_raw` row has a Loro node once the scan is quiescent — the D175.a
  property, read through `sql_only_diagnostics`;
- adopted blocks' sibling order equals file order (`inv-live-children-match-ref`);
- adopted block IDs are unchanged (invariant 13 — no re-mint);
- **the undo journal records nothing for an adoption wave** (D115.a: undo is
  journal-owned; adoption is not user intent, and a Ctrl-Z after boot must not
  un-adopt blocks);
- **org files are byte-identical across `Reboot → Reboot`** (the write-back
  echo risk: an adopted block's DiffEvent must not re-render the file and
  change its `content_hash`, or every boot re-ingests);
- no duplicate stable ID in the tree (§2c, once Inc 0's warn is loud).

### 4.2 A new transition — only if the reboot case cannot reach the freeze-in

§4.3's second-boot freeze-in is what pins Inc 4. **First check whether the
reboot case already reaches it**; add `VaultWithPreLoroRows` only if it does
not. If it is needed, the catalog gap is real and documented: `E2ETransition`
(`crates/holon-integration-tests/src/pbt/transitions/mod.rs:260-348`) has no
shape that seeds SQL rows without Loro nodes, and
`crates/holon-integration-tests/tests/loro_suite/loro_restart_unseeded_vault.rs:35-37`
says so outright (*"no restart transition in keystone"*).

Design constraint if it is written:

> The dedicated test reaches the state by flipping `enable_loro(false)` →
> populate → reopen with Loro on, which trips the invariant-10 epoch guard and
> needs `HOLON_CONSOLIDATOR_MIGRATE=1`
> (`loro_restart_unseeded_vault.rs:10-16`, `:64`). That acknowledgement is an
> **interim wipe-and-reseed** (Model.md:115-121) — it deletes durable state,
> i.e. exactly the rows the test exists to observe.

**So the transition must not flip the mode.** It seeds `block_raw` rows
directly, with no Loro node, under a single consolidator epoch. That is both
faithful to the production state (the rows are there; how they got there is
history) and the only expression that survives the epoch guard. Per the
standing directive, the generator/ref/SUT are shared with the keystone rather
than written fresh; `loro_restart_unseeded_vault.rs` supplies the fixture
content, not the mechanism.

It carries §4.1's oracle list unchanged.

### 4.3 Reaching §2b's freeze-in needs a second boot

`Reboot → Reboot` (over the reboot case, or over `VaultWithPreLoroRows` if
§4.2 proved necessary) is what makes the cold-boot fast
path (`file_sync_controller.rs:3282`) observable: boot 1 adopts and re-stamps
the content hash under the Loro tag, boot 2 skips the file and the residue
stays orphaned. A single-boot case cannot go red on Inc 4.

Two obstacles to state in the PR rather than discover:

- `Reboot` is **off by default**, env-gated on `HOLON_PBT_REBOOT`, and weighted
  rare even when on (`reboot.rs:56-68`, `:95`). The lane must run it
  explicitly.
- `SimulateRestart` (`transitions/mod.rs:320`) is **not** a substitute: it only
  re-parses org files against the still-running controller
  (`reboot.rs:26-29`), so it never restarts the store and cannot exercise the
  cold-boot fast path.

### 4.4 Green criterion

`reboot-orphans-the-previous-boots-watchers` green **10/10** over `just
hand-authored` with its skip default removed (deterministic replay per the
standing directive — hand-authored regressions, never proptest seeds), and the
D175.a oracle green across the `Reboot → Reboot` sequence.

---

## 4a. Inc A — the `Consolidator` seam

**No behaviour change.** Existing paths are re-expressed as calls through the
trait at exactly the points D175.a will touch.

### Where it lives

**`holon-core`.** Not `holon-api`: the trait's associated `Version`, `Delta`,
`Seen` and `Ops` are domain vocabulary, and `holon-core` is where
`BlockOrdering` (`crates/holon-core/src/block_ordering.rs`) and
`EntityCellRegistry` already sit — the two traits the Loro adapter already
implements, so the new seam joins its own family rather than founding a second
one. `holon-api` is barred by the `api-storage-backend` / `api-frontend-dep`
arch-lint rules from importing a storage backend (Model.md:257-260), and while
the trait itself would not violate them, the `Delta`/`Ops` types it needs are
already in `holon-core`. Put `EntityUri`-level vocabulary that `holon-api`
already owns (`EntityUri`, `FieldId`) where it is.

### The three traits, as study §7 defines them

`Consolidator` (study §7:370-404), `Replica<C>` (§7:406-411), `TextFormat`
(§7.1:464-486) — **verbatim from the study**, with its two corrections applied:

- **`Entity`/`EntityUri`, never `Block`** (§7:414-425). `Delta` is per entity,
  `ever_seen` takes an `EntityUri`, `carries` is per `(EntityKind, FieldId)`.
- **No `line_span`** (§7.2:531). The delta lift is **parse-and-diff**, which is
  what the org ingest already does — `new_blocks_vec` against `old_blocks`
  (`file_sync_controller.rs:3875-3887`, `:4186`, `:4207`) *is* a parse-and-diff
  keyed by `EntityUri`. No second diff engine.

`GitConsolidator<F: TextFormat>` is **declared, not implemented** (study
§7.2:547-548).

### The Loro implementation: a thin adapter, with one real question

Every method but one maps onto something that exists:

| Trait method | Loro today |
|---|---|
| `Version` / `is_ancestor` | Loro frontiers |
| `epoch_id` / boot check | `guard_consolidator_epoch`, `crates/holon-app/src/consolidator_epoch.rs` (invariant 10) |
| `diff(from, to)` | the outbound projector's base diff, `LoroSyncController::on_loro_changed` |
| `apply` | `create_block_with_properties` / `update_block_position` / `set_block_*` on `LoroBackend` |
| `register_base` / `release_base` | retention; **ruling 3** makes Turso a registered base, which is what keeps tombstones alive (invariant 9) |
| `merge` | Loro's own merge; the rung is op-CRDT |

> **MEASURED 2026-09-22 (Inc A) — the "no new index" claim below is REFUTED
> for any store that has been compacted.** Production saves a shallow snapshot
> on a schedule (`LoroDocument::export_compact_snapshot`,
> `crates/holon-loro/src/loro_document.rs:333`). Probe
> `a_tombstone_survives_a_compacted_snapshot`
> (`crates/holon-loro/src/block_cell_registry.rs`): create → delete → shallow
> snapshot → import. The deleted node **survives** (`parent = Deleted`) but its
> meta is **empty** — `["Deleted id=None"]` — so the stable id no longer maps
> to the tombstone and `ever_seen` answers `Never`. Log:
> `lane-logs/incA-compaction-probe-1.log`. Consequence: **Inc B cannot rely on
> the tree's tombstones.** A durable deleted-id record is required first; the
> recommendation is a `LoroMap` of deleted stable ids written in the same commit
> as the delete (live state, so it survives shallow snapshots and syncs to peers).
> That is an architecture choice and goes to Martin.

**`ever_seen` — answered for an uncompacted store.** The Loro API that
answers it, in the pinned fork (`loro = "= 1.13.9"`, git rev `6f5b2d7e`;
`crates/loro/src/lib.rs` in the checkout):

- `LoroTree::get_nodes(with_deleted: bool) -> Vec<TreeNode>` — `:3173`,
  *"Return all nodes, if `with_deleted` is true, the deleted nodes will be
  included."*
- `LoroTree::contains(&TreeID)` — `:3154`, *"Return whether target node
  exists. **including deleted node**."*
- `LoroTree::is_node_deleted(&TreeID) -> LoroResult<bool>` — `:3163`, already
  wrapped locally as `node_deleted_now` (`loro_backend.rs:1237`).

So `ever_seen(id)` = scan `get_nodes(true)`, read each node's stable id from
its meta (`read_stable_id`, `crates/holon-loro/src/settled_read.rs:26`; a
tombstoned node's `get_meta` still returns `Ok` — already known here,
`loro_backend.rs:1003-1006`), then:

```
stable id absent from the scan        → Seen::Never
present, is_node_deleted == false     → Seen::Live
present, is_node_deleted == true      → Seen::Deleted(version)
```

**Two things a `Deleted(version)` cannot yet supply, and what to do:**

1. **The `Version` of the delete.** `is_node_deleted` is a current-state
   predicate; it does not say *when*. Ruling 2 needs only "is it deleted", not
   "when", so **Inc A's `Seen::Deleted` carries the consolidator's current head
   as a conservative stand-in**, and the variant is documented as such. If a
   later rule needs the true delete version, that is when a history index gets
   built — and the place is `crates/holon-loro/src/loro_backend.rs` beside the
   stable-id cache, not a new crate.
2. **Tombstone durability.** A tombstone answers `Deleted` only while history
   is retained; after a GC or a shallow snapshot it degrades to `Never` —
   silently resurrecting the block. This is invariant 9 and it is exactly what
   ruling 3 (Turso as a registered GC base) exists to prevent. **Inc A must
   assert the registration**, not assume it: if Turso is not a registered base
   at the point `ever_seen` is first called, that is a loud error, because
   `Never` would then be a lie.

### The keystone model implementation

Trivial linear history: a `Vec<Version>` and a map `EntityUri → Live |
Deleted(at)`. It is the second implementation that proves the trait is not a
Loro shape with extra words, and it is what lets Inc B's oracle run in the
model as well as the SUT.

### Which call sites move behind the trait, and which do not

**Move (these are where D175.a writes):**

| Site | Becomes |
|---|---|
| `file_sync_controller.rs:4186-4194` — `needs_reseed`'s `in_tree(id) == Some(false)` | `ever_seen(id)`, three-way |
| `file_sync_controller.rs:4655-4681` — V1, the place-loop skip | deleted in Inc C2; until then, reads the trait |
| `block_cell_registry.rs:788-812` — V2, `live_children`'s `None` | deleted in Inc C1 |
| `file_sync_controller.rs:1514-1523` — `content_present_in_all_stores` | `ever_seen` per member entity (Inc C4) |
| `block_ordering.rs:208` — `in_tree`, whose `Ok(Some(false))` is the pre-Loro signal | **subsumed** by `ever_seen`; `in_tree` is deleted, not kept alongside — two existence predicates is the bug class this plan closes |

**Stay (not existence questions; moving them is scope creep):**

- `BlockOrdering::place` / `place_all` / `prev_sibling` / `first_child` —
  ordering, not existence.
- `create_in_tree` / `create_in_tree_batch` — the *write*. `Consolidator::apply`
  is its eventual home, but re-routing the create path is a second refactor and
  Inc A is "no behaviour change".
- `EntityCellRegistry::write_field` and every cell backing — field writes.
- The write-back / org-render path.

### Inc A's own gate

No behaviour change means: the full suite is green with **zero** test edits
beyond the model impl. If a test needed changing, the adapter was not thin.

## 4b. Inc B — C1 as an assertion

**Rule.** Adoption calls `ever_seen` and branches, with no fourth case:

| `ever_seen` | Adoption |
|---|---|
| `Never` | **Adopt.** ID preserved, placed by file order (§1.5). |
| `Live` | No-op — already there (`create_entity` is idempotent, `block_cell_registry.rs:550-554`). |
| `Deleted(_)` | **Refuse, loudly** — unless ruling 2's file base proves resurrection. |

**Ruling 2's resurrection rule.** A `Deleted(_)` entity is resurrected iff the
file base proves the user re-added it: `X ∉ file base ∧ X ∈ file now`. When
`X ∈ base`, the delete wins and the row is a projection lag, not a block.
The file base is the same `old_blocks` the diff loop already holds
(`file_sync_controller.rs:3875-3887`), so this rule needs no new state — it
needs the branch to *consult* it, which today it does not.

**Red-first test (study §2:118-123): crash between delete and projection.**

> *"Between a Loro delete op and its projection into Turso, a crash leaves a
> row Loro no longer has. On the next boot A's rule reads that row as a create
> candidate and resurrects the deleted block."*

The case: delete a block, suppress the projection (crash/kill before the
`block_raw` DELETE lands), reboot. **A row whose id Loro has deleted must NOT
be adopted.** Red before Inc B: the row is `Never`-shaped to a current-state
check and gets resurrected. Green after: `ever_seen` answers `Deleted(_)`, the
file base does not contain it, adoption refuses.

This test is the reason Inc B precedes the veto deletions: **Incs C1–C2 make
adoption happen, and without Inc B they would make this resurrection happen
too.**

---

## 5. Increments

Each independently landable, each green before the next.

| # | Change | Gate |
|---|---|---|
| **A** | §4a: `Consolidator` / `Replica` / `TextFormat` in `holon-core`; Loro adapter (incl. `ever_seen` over `get_nodes(true)`); keystone model impl; `GitConsolidator<F>` declared only. The named call sites move behind the trait; `in_tree` is deleted, not kept beside `ever_seen`. | **Zero behaviour change** — full suite green with no test edits beyond the model impl. Asserts Turso is a registered GC base before `ever_seen` is trusted. |
| **B** | §4b: adoption branches on `ever_seen`; `Deleted(_)` refuses loudly; ruling 2's file-base rule decides resurrection. | **Red first**: crash-between-delete-and-projection resurrects a deleted block. Green after. |
| **C0** | Promote the duplicate-stable-ID warn (`loro_backend.rs:1404-1413`) from `HOLON_LORO_DUP_DEBUG`-gated to unconditional `error!`. | Existing suites stay green; no new red. |
| **C1** | Un-skip `reboot-orphans-the-previous-boots-watchers`: delete the default from `justfile:257` and the `KeystoneKnownReds.md` row. Add §4.1's oracles. §4.2's transition **only if** the case cannot reach §4.3's second boot. **Red, logged.** | Red for the right reason: 3/10, `loro_node=Never` rows on the strict three-way invariant. |
| **C2** | Delete V2: `live_children` resolves the parent instead of answering `None` (`block_cell_registry.rs:788-812`) — the existence question now goes to `ever_seen`. | C1 moves toward green; `inv-live-children-match-ref` stays green. |
| **C3** | Delete V1: `file_sync_controller.rs:4655-4681` collapses to the existing `bail!`, adopting through `Consolidator::apply`. **Plus the mount/pairing half of the ruling-4 guard** — this is the first increment that adopts, so the guard cannot trail it into a shipped build. **Precondition: the §3 measurement has been run.** | C1 green for single-boot cases; per-file failure still falls through to `arm()`; no adoption while a mount or pairing is live. |
| **C4** | (b1): widen `content_present_in_all_stores` to the file's full member set, asking `ever_seen` per entity (`file_sync_controller.rs:1514-1523`). **First step: the membership grep below.** | C1's `Reboot → Reboot` sequence green. Measure boot cost, release profile. |
| **C5** | The home-file-holder half of the ruling-4 guard (single adopter). | A two-peer adoption case goes green; ADR 0033's mount refusal unchanged. |
| **C6** | *Conditional on §3's measurement and **D178**.* (b2) background pass + `VaultBlocksBeingAdopted` condition. | Not written if step 3 of §3 returns 0. |

**Incs A–C4 do not wait on D178 or D179.**

- **D178** (background pass) only gates C6, which is conditional anyway.
- **D179** (SqlOnly: migrate-only vs Turso tombstones) is absorbed by Inc A's
  shape, not by a branch in it: study §7:386-388 says a Turso consolidator
  *cannot* answer `ever_seen` without added tombstones, **and the trait makes
  that gap explicit**. So under either outcome Inc A is unchanged — a
  `TursoConsolidator` either is never written (migrate-only) or is written
  later with a tombstone table behind the same `ever_seen`. Everything in this
  plan is already gated on `Consolidator::Upstream`, so SqlOnly reaches no
  adoption code either way.

### Where ruling 1's Honour/Quarantine policy plugs in

Out of scope for this plan. The seam is **the guard immediately before the
consolidator write** — the same branch Inc B installs for `Deleted(_)`, on the
delete side rather than the adopt side: `Consolidator::apply` is the single
writer (study §7:402-403), so a delete-amplitude policy is one predicate
evaluated on the `Delta` before it is applied, with Honour as the default arm
and Quarantine as the other. Inc B's refusal branch is where a later increment
adds it; nothing else in this plan needs to know it exists.

### Amendment 1 — (b1) rests on a hypothesis; test it first

§2b says the file's block membership "is already persisted beside
`file.content_hash`", citing `persisted_read_only_blocks`
(`file_sync_controller.rs:3312`) — but that is the **read-only** case. That a
*writable* file persists the same membership is a **hypothesis, not a verified
fact**. Inc 4's first step is therefore:

```
grep -n 'read_only_blocks\|persisted_read_only_blocks\|block_membership' \
  crates/holon-filesystem/src/file_sync_controller.rs
grep -rn 'read_only_blocks' crates/holon-turso/src/schema_modules.rs   # is the column general or read-only-scoped?
```

If writable files carry no membership, Inc 4 **adds it**: one column on the
`file` row holding the block IDs the ingest declared, written by the same
UPDATE that stamps `content_hash` (`file_sync_controller.rs:5034`, `:5100`) so
a matched hash and a stale membership are unrepresentable — the same
one-UPDATE argument the read-only path already makes at `:3297-3311`. That
schema change is part of Inc 4, and the increment is not "widen a predicate"
but "persist the membership, then widen the predicate".

### Stays out of scope

- The consolidator-epoch migration (invariant 10) — adoption is not a mode flip.
- Tombstones / GC (invariant 9).
- Anything about `sort_key` becoming a writable field (invariant 3 forbids it).
- SqlOnly mode: `consolidator() != Upstream` means there is no second tree, and
  every branch here is already gated on `Consolidator::Upstream`.
- The `write_field("content")` fallback (`block_cell_registry.rs:916-922`) —
  same family, but it is a *write* path and adoption makes it dead rather than
  wrong; delete it in a follow-up once §3 shows R = 0 steady-state.

### Risk register

| Risk | Where it bites | Mitigation |
|---|---|---|
| **The trait adapter changes a hot path's cost** | `ever_seen` over `get_nodes(true)` is O(nodes **incl. tombstones**) per call. The predicate it replaces (`is_live_anywhere`) is already O(nodes) and was made affordable only by `warm_stable_id_cache` once per chunk (`block_cell_registry.rs:694-713`, which documents the quadratic it fixed). A per-block `ever_seen` reintroduces that quadratic — and worse, because tombstones grow the scan monotonically. | The Loro adapter caches `ever_seen` behind the same warm-once-per-chunk discipline, keyed by stable id, holding all three states; the cache is warmed from one `get_nodes(true)` pass. Inc A's gate must include a boot-ingest timing over the largest fixture, **release profile**, compared against the pre-Inc-A number — a trait refactor that silently doubles cold-boot is a failure even with every test green. |
| **`ever_seen` degrades to `Never` after GC** | A tombstone answers `Deleted` only while retained; a GC or shallow snapshot turns it into `Never`, which silently resurrects the block — the exact bug Inc B exists to stop, reappearing through the back door. | Ruling 3 (Turso as a registered GC base) plus invariant 9. Inc A **asserts** the registration before `ever_seen` is trusted rather than assuming it. |
| **Write amplification at boot** | `R` Loro ops → `R` DiffEvents → `R` SQL writes in one wave; the `finish_initial_scan(30_000)` convergence wait (`di.rs:1196`) could hit its ceiling on a large `R`. | Chunked through `create_in_tree_batch`. **Measuring `R` (§3) is a hard precondition of Inc 3**, the first adopting increment. If the wave threatens the 30 s ceiling, Inc 6's background pass (outside that wait) takes it instead. |
| **Sync peers fork the block** | §2c — silent LWW at `loro_backend.rs:1414`. | Inc 0 makes it loud. The mount/pairing refusal ships **with Inc 3**, so no build that adopts ever ships without a guard; Inc 5's single-adopter half follows under D178. |
| **Undo journal** | D115.a: undo is journal-owned. Adoption ops are not user intent and must not become undoable, or Ctrl-Z after boot un-adopts blocks. | Adoption runs under an ingest-class origin, like the existing reseed; assert the journal records nothing for an adoption wave. Add this as an oracle on §4.2's transition. |
| **File write-back echo** | An adopted block emits a Loro op → DiffEvent → SQL write → the write-back loop may re-render the org file, whose new bytes change `content_hash`, which on the next boot forces a re-ingest. A loop is possible if adoption is not idempotent. | `create_entity` is idempotent on a live node (`block_cell_registry.rs:550-554`) and `update_block_position` no-ops when already positioned (`file_sync_controller.rs:4618-4620`), so wave 2 is empty by construction. Pin it: the §4.2 transition must assert byte-identical org files across `Reboot → Reboot`. |
| **V1 removal turns a soft skip into a file failure** | A malformed vault file now fails its file. | The driver already collects per-file failures and arms the watcher anyway (`di.rs:1239-1252`); the failure surfaces as the existing degraded banner. |
| **`heal_title_less_doc_roots` ordering** | Inc 6 would run after it; a title-less root healed into existence must be adoptable in the same boot. | Place Inc 6's pass strictly after the heal sweep (`di.rs:1210`) and before `finish_boot_seeding()` (`di.rs:1220`). |

### Staleness guard greps

Before starting, and again before landing each increment — a mismatch means
re-read, not re-plan-from-memory:

```
grep -n 'ALLOW(fallback)' crates/holon-filesystem/src/file_sync_controller.rs        # expect 478, 4670, 4839
grep -n 'ALLOW(fallback)' crates/holon-loro/src/block_cell_registry.rs               # expect 538, 794, 913
grep -n 'needs_reseed' crates/holon-filesystem/src/file_sync_controller.rs           # expect 4186, 4195
grep -n 'content_present_in_all_stores' crates/holon-filesystem/src/file_sync_controller.rs   # expect 1514, 3282
grep -n 'async fn create_in_tree' crates/holon-core/src/block_ordering.rs            # expect 161
grep -n 'heal_title_less_doc_roots' crates/holon-orgmode/src/di.rs                   # expect 1210
grep -n 'LORO_DUP' crates/holon-loro/src/loro_backend.rs                             # expect 1404, 1408
grep -n 'reboot-orphans' justfile                                                    # expect the skip default at :257 — Inc 1 deletes it (§4.1)
grep -rn 'sql_only_diagnostics' crates/holon-integration-tests/src/pbt/invariants/bodies/view_model_matches_store.rs   # the loro_node=Never signal; ABSENT means the base is pre-F1a (§4.0)
grep -n 'HOLON_CONSOLIDATOR_MIGRATE' crates/holon-integration-tests/tests/loro_suite/loro_restart_unseeded_vault.rs  # the epoch guard a new transition must avoid (§4.2)
grep -n 'read_only_blocks' crates/holon-filesystem/src/file_sync_controller.rs       # Inc C4 first step: is membership general or read-only-scoped?

# Inc A — the ever_seen substrate. All three must be present in the pinned fork:
grep -n '^loro = ' Cargo.toml                                                        # expect = 1.13.9, git rev 6f5b2d7e
L=$(ls -d ~/.cargo/git/checkouts/loro-*/*/crates/loro/src/lib.rs)
grep -n 'pub fn get_nodes\|pub fn contains\|pub fn is_node_deleted' $L               # expect 3154, 3163, 3173 (checkout 6f5b2d7)
grep -n 'fn node_deleted_now\|read_stable_id' crates/holon-loro/src/loro_backend.rs crates/holon-loro/src/settled_read.rs
grep -n 'warm_stable_id_cache' crates/holon-loro/src/block_cell_registry.rs          # the caching discipline ever_seen must reuse (risk row)
grep -rn 'fn in_tree' crates/holon-core/src/block_ordering.rs                        # Inc A DELETES this; a survivor means two existence predicates
```

---

## Open items — D178 and D179 (Incs A–C4 do not wait on either)

1. **D178 — the background pass.** Measure first. Recommendation stands: do not
   write (b2) until §3 step 3 shows rows with no live home file. Gates C6 only.
2. **D179 — SqlOnly: migrate-only vs Turso tombstones.** Inc A is the same
   under both outcomes (study §7:386-388: the trait makes Turso's missing
   `ever_seen` explicit rather than papering over it). Every path here is
   gated on `Consolidator::Upstream`, so SqlOnly reaches no adoption code
   either way.

Ratified and folded in: delete-amplitude Honour-by-default (seam named in §5),
file-base resurrection (Inc B), Turso as a registered GC base (Inc A's
assertion), single adopter = home-file holder (C3/C5), and build the
`Consolidator` abstraction now (Inc A). Measured R9/R10 — no authored field is
SQL-only — is why adoption reads the org file and loses nothing.

Base for implementation: **post-F1a main**, from a fresh worktree, SHA to
follow.
