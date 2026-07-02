# 0008 — Closing the gaps to the Model.md target architecture

**Status: proposed (2026-07-01).** Companion to
[docs/Architecture/Model.md](../../docs/Architecture/Model.md) (invariants 1–12)
and successor-in-spirit to spec 0007, which already tracks: sole Turso writer,
authority-first delete, intent-vocabulary flip (stop decoding `sort_key`,
Phase 5), and total verbatim fi projection. **This spec does not duplicate
those items** — it plans only the gaps Model.md added (invariants 8–12 plus the
merge-fidelity ladder), ordered by risk-reduction per effort.

Verified baseline: repo @ `ypytzkwy 958e8da8` (2026-07-01 senior review).

---

## Phase 0 — decisions and docs (hours, no code)

**0.1 Fold command sourcing into Replication §7.**
The upstream intent channel *becomes* the future durable command log;
`id_mappings` is the `OwnForeign(map)` capability of Replication §2. Move the
still-relevant content of `crates/holon/src/storage/command_sourcing_todo.md`
into Replication §7 as "the durable form of this channel (offline, future)";
shrink Storage.md's banner'd section to a pointer. Deletes two of the three
overlapping offline designs (OperationLog's `PendingSync`/`Synced` statuses
stay — they're the third, and implemented).

**0.2 Decide the mixed-origin sibling-set rule.**
Promote Replication §10's deferred question to a decided rule: *order
ownership is per parent, assigned to the parent's home component; foreign
children get their fi minted by that owner.* Doc edit only; implementation
waits for the first external integration that embeds under a note.

**0.3 Document the file-syncer contract (invariant 11).**
User-facing vault docs + README: a vault directory must not be under
Syncthing/iCloud/Dropbox while Loro P2P is active; cross-device convergence
goes through Loro. (Enforcement is 1.2.)

## Phase 1 — fail-loud guards (small code, ~1–2 days)

**1.1 Consolidator epoch marker (invariant 10).**
Persist the consolidator identity (e.g. `consolidator: loro|sql` + peer id) in
`.holon/` at first boot. At startup, compare with the effective config:
mismatch → **hard error** naming the invariant and the two modes (no silent
LWW-over-incomparable-timestamps, no fi-keyspace mixing). Escape hatch:
`HOLON_CONSOLIDATOR_MIGRATE=1` acknowledges the flip and (until 4.1 lands the
real migration) wipes bases + Turso so everything re-seeds from the new
consolidator. Fail-loud beats fake-working.

**1.2 Foreign-file-syncer tripwire (invariant 11).**
Vault scan at startup + watcher-time check for `*.sync-conflict-*`,
`* (conflicted copy)*`, `.icloud` artifacts → disclosed error (startup) /
`tracing::error!` + skip-ingest (runtime). Cheap detection of the worst
outcome (conflict copies ingested as duplicate-ID documents).

**1.3 Archlint smell: fi minting outside the order owner (invariants 2/10).**
`gen_key_between` / `default_sort_key` call sites allowed only in the
sanctioned order-owner modules (block_ordering, loro_seams, sql SqlOnly path,
tests). Guards the keyspace-mixing bug class the same way `sole_block_writer`
guards raw writes.

## Phase 2 — kill the `write_field` carve-outs (invariant 12; = Cells plan Phase 2)

Land the missing backings, each deleting a carve-out in
`crates/holon-loro/src/block_cell_registry.rs`:

- **2.1** ✅ `LoroMetaCellBacking<T>` (scalar fields on the tree-node meta map,
  T ∈ {bool, i64, String, Value}) — unblocks the `completed`-style cell call
  sites Operations.md documents; `write_field`'s generic scalar arm now routes
  through the cell. `crates/holon-loro/src/loro_meta_cell_backing.rs`.
- **2.2** ✅ `LwwScalarBacking<T>` (SqlOnly twin), so both modes present the same
  cell surface. Landed as the backing type + unit tests
  (`crates/holon-core/src/cell.rs`); SqlOnly registry wiring (entity-cache read
  + CDC signal) is deferred, exactly as SqlOnly `content` cells are.
- **2.3** `LoroTreeParentCellBacking` / `LoroTreePositionCellBacking` last —
  they overlap the spec-0007 intent-vocabulary work; sequence after its
  Phase 5 flip to avoid building on the `sort_key` fallback.

Exit criterion: `write_field`'s field-name `matches!` shrinks to the four
no-Loro-encoding fields; everything else resolves a backing.

## Phase 3 — complete the merge-fidelity ladder (medium)

**3.1 Wire `TransientTextMergeProvider` into the no-store conflict path.**
Today a file-edit racing a UI edit in SqlOnly resolves by LWW (whole-value).
The ladder says it should degrade to base-3-way, not LWW: the org diff already
has the base; feed (base, disk, mine) through the transient `LoroText` and
write the merged text. Disclose with a `tracing::info!` merge note.

**3.2 Make the Loro path unable to mint order keys (type-level). — DONE**
`split_block` calls `new_child_anchor` in both modes; the Loro impl returns a
placeholder that `apply_create` overwrites. Split the trait so the Loro-mode
ordering seam has no `-> String` key-minting method at all (mode-specific
impl, per Replication §5). Removes the "placeholder that works by convention"
in `loro_seams.rs`.
Landed: `new_child_anchor` moved off `BlockOrdering` onto a new
`OrderKeyMinting` trait (holon-core/block_ordering.rs), implemented only by
`SqlBlockOperations` (the `Store` order owner). `split_block` reaches it via
the new `BlockOperations::order_key_minter()` seam on its SqlOnly branch only.
The `loro_seams.rs` placeholder impl is deleted; `LoroBlockOrdering` can no
longer mint by construction (compile-level witness in loro_seams.rs tests).

## Phase 4 — teeth and the real migration (later, sequenced behind the above)

**4.1 Consolidator handover migration.** Replace 1.1's wipe-and-reseed escape
hatch with the real thing: seed the new consolidator from the old consolidated
state, rewrite every replica's base, bump the epoch marker. Needed before the
"Loro becomes the default durable base" flip (Replication §2).

**4.2 PBT coverage, per the north-star (caps into the ONE composed PBT, no new
slices):** (a) a stale-base resync transition asserting no
resurrect-after-delete (invariant 9's observable consequence — also the
tombstone-GC precondition check, since no GC exists yet, this pins the
behavior *before* anyone adds GC); (b) an epoch-flip transition asserting the
1.1 hard error fires.

## Explicitly not planned

- Splitting `HOLON_CRDT_ENABLED` into per-axis env vars — YAGNI; the axes are
  separated conceptually in Model.md, config stays one switch.
- Tombstone GC itself — nothing GCs tombstones today; invariant 9 only
  constrains whoever adds it (4.2a pins the behavior).
- Version vectors / arbitrary P2P topology — per Replication §6, steal
  (Loro/jj) if ever needed.
