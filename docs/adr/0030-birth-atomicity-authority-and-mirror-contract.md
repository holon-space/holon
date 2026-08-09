# ADR 0030 — Birth atomicity: transitions fire in the authority; mirrors converge under their own contract

Date: 2026-08-08. Status: accepted (Martin, after senior-review with codebase validation;
amended same day per an independent second review that verified every cited mechanism
against the code — amendments corrected claims of fact and scope, no Decision reversed).
Companion to ADR 0029 (identity minting) and ADR 0031 (the Holon-native transition
catalog, which records the ratified D7 decision — this ADR's contract is designed to
fold into that catalog).

## Problem

Entities have repeatedly been observable in states that satisfy no writer's intent:
nodes persisted without ids ("half-born", withheld with their subtrees forever), rows
durable but unaddressable (mounts), 15-byte org files crashing readers, a create()
that errs after its node is written. The 2026-08-06 ruling closed *observation*
atomicity within one Loro doc (Layer-2 per-doc RwLock + WriteTxn) but deliberately
did not cover *birth* atomicity: a creation ending half-done across steps, stores
(Loro + SQL + junctions + files), and crashes. Prior analysis framed cross-store
births as needing saga machinery (durable intent + reaper). That framing conflated
two different things.

## Decision

**D1 — A birth is a Petri-net transition, and it fires atomically in exactly one
authority store.** A transition either fires — guard satisfied, output marking
produced — or it does not; there is no half-fired transition. Concretely: the birth
contract (every required facet: id, content, marks, order, tags, …) is the
transition's guard, validated in full BEFORE any write; the firing is one write
transaction in the entity's authority store. Creates are total, or they are refused
with zero side effects. This is one instance of the general rule that user-intent
operations are PN transitions; the enforcement point is the reification machinery of
ADR 0031 (macros generate guard-then-fire, so an author cannot write a partial birth by
accident). Until that machinery's guard gate lands, the dispatcher's create arms carry
the contract manually.

Two qualifications. (i) "Zero side effects" is enforced by the firing transaction's
rollback; any step that writes OUTSIDE that transaction is part of the firing and must
move inside it or be independently harmless. The two known violations are REMEDIATED
(see Enforcement → Order-mint atomicity): the order-mint rebalance no longer writes at
all, and one placement is now one transaction. (ii) Guard evaluation and firing
are atomic only under the one-consolidator-per-vault serialization (Model.md Layer 2);
the guard's reads (sibling scan, holder recognition) are valid at fire time by that
assumption, not by isolation. Deferred-FK validation at commit with rollback is a
blessed guard mechanism — "total or refused" is delivered by the transaction, not by
literal guard-before-write, and that is exactly why pre-transaction writes violate D1.

Scope: D1 binds locally-initiated firings. Sync-delivered markings (iroh/HTTPS) are
trusted verbatim — the guard ran (if at all) at the origin peer; cross-version
contract drift on synced births is an open question of the sharing contract, not of
this ADR.

**D2 — Mirroring is orthogonal to birth.** A creation needs to be observed only in
the authority. Propagation to other representations (SQL projections in Loro mode,
org files, matviews) is the mirroring layer's concern; a mirroring failure is a
mirroring defect, never a violation of the birth transition's contract. There is no
cross-store birth saga: mirrors are (for store-owned facets — see D5) re-derivable
from the authority, so **re-projection is the repair mechanism** — with one scoping
rule and one disclosure. Scoping: for org files, a divergence is by DEFAULT inbound
intent (the Layer-1 replica contract, Model.md — `on_file_changed` 3-way-merges the
diverged file INTO the store), not damage; repair-by-re-derivation applies only where
an ownership proof shows the divergence is not user intent (the store-wins /
`doc_home` proof machinery) or where the mirror is a declared one-way sink.
Disclosure: the general divergence-detect-and-re-project loop (file and SQL) is
UNBUILT today; what exists is forward propagation (`on_block_changed`), missing-file
materialization (`materialize_page_identity_file`), and single-purpose healers. The
authority remains the durable record of what must exist; no per-birth intent log is
needed.

**D3 — The mirroring contract.** Two guarantees keep half-born states from
re-entering through the mirror door:

1. **Per-authority-commit atomic application.** A mirror may lag; it must never show
   a between-commit state. Turso IVM's per-transaction deltas already give this for
   SQL-fed views. File mirrors have no transactions, so file write-back MUST use
   atomic replacement (write temp, rename) — see Enforcement; an in-place write that
   can tear is a contract violation, and a torn file is worse than a stale one
   because org files are also an ingest source: a torn mirror re-ingested becomes
   authority corruption. This clause covers every non-transactional durable mirror,
   not only org files. Known inventory today: org write-back (`fs_port.rs` —
   satisfied, see Enforcement), sync-base JSON sidecars (`sync_base_store.rs` —
   satisfied through the same helper; torn bases feed the 3-way diff exactly as
   torn org files feed ingest), share snapshots and roster sidecars (audited —
   already tmp+fsync+rename), the content-hash / `doc_home` persistence. Anything writing a durable derived artifact is in scope by
   default; exemptions must be argued, not assumed. (The Loro snapshot writer already
   complies via `write_atomic`, `loro_document.rs:274` — the authority writer got
   this right; the mirror writers didn't.)
2. **Divergence is disclosed and repaired by re-derivation, never persisted
   silently.** The existing store-wins stance and the file-sync ownership-proof
   machinery are instances of this.

**D4 — Two-authority births order their firings leak-safely.** Where one birth
touches two authorities (identity mint in SQL per ADR 0029, then structure in the
block authority), the crash window between the firings must leak in the harmless
direction: **mint first**. Today the mint executor persists nothing (recognition is
a read plus a pure decision, `sql_operation_provider.rs:1971`; unique-random is pure
generation), so the window currently leaks nothing at all; the ordering rule exists
so that if minting ever becomes a durable registration, the leak stays a harmless
unused id. Same-title convergence is carried by derived-id determinism plus
recognition (`bless_carried_*`, `identity_minting.rs:330`), NOT by mint-side
reservation — the mint arbitrates nothing; collisions are settled at the INSERT
(`ON CONFLICT`) and by CRDT merge in the Loro arm. The current create paths already
order this way (create arm `sql_operation_provider.rs:2696` mints before the
transaction at `:2760`; `create_page_from_link` `:3100`; `block_to_page_plan` mints
in a read-only planner phase `:3455`; template instantiation pre-mints all node ids,
`template_instantiation.rs:319`); the Loro arm takes only pre-minted ids, enforced by
the ADR 0029 lint rather than code shape. The reverse leak — structure without
minted identity — is a defect, and reordering that way is forbidden.

**D5 — Authority is per (entity type, mode, facet); the declaration is owed, not yet
written.** Not per type alone:
the block authority flips between Loro and SQL by mode, and — decisively — org
files carry authored facets the store cannot represent (file-level property
drawers, non-Holon drawer keys, preserved verbatim per
`file_level_property_drawer.rs`). For those facets the FILE is the authority and
the store is not involved. Consequently repair-by-re-derivation (D2/D3.2) is scoped:
it may regenerate only store-owned facets and must preserve file-authoritative
ones. A "mirror" is a mirror only facet-wise; blind full-file regeneration from the
store is a data-loss bug, not a repair. Today the split exists only as
parser/renderer behavior (`models.rs:53-57`, `:853-913` — no registry answers "who
owns this facet"), so repair-by-re-derivation for file mirrors is BLOCKED on reifying
this declaration (target: the ADR 0031 catalog — whether it folds in there is ADR 0031's
deferred-open ruling P4). Until then, no automated file repair beyond the existing
proof-gated paths may land.

## Enforcement

- **Guard-then-fire:** the ADR 0031 macros; until their guard gate lands, create arms validate
  the full birth contract before the first write (the marks F3 fix is the
  pattern). Keystone invariant target: no observer ever sees an entity that fails
  its type's birth contract.
- **Fault-injection PBT:** inject failures between mint and create, and between
  authority commit and mirror application; the only observable end states are
  "never existed" and "fully born (mirrors eventually converged)". This is the
  checkable form of "no permanent loss". It additionally requires an ordering
  guarantee D3 does not yet state: a mirror application must not become durable
  before the authority commit it derives from is durable (Loro authority durability
  is the snapshot save, `loro_document_store.rs:196`; org files and SQL projection
  currently have no such ordering). Until that holds, a crash can yield a third end
  state — the birth reborn from the org mirror via boot ingest, carrying only
  file-representable facets, with store-only facets silently gone — which the PBT
  must either forbid (once the ordering lands) or classify as the disclosed
  file-rescue outcome, never pass silently. Note the saga rejection leans on this:
  the intent log is redundant with the authority only when the authority is the
  first thing to become durable.
- **Atomic file replacement:** SATISFIED for every write through the port.
  `FileSystem::write` now *contracts* atomic replacement and both adapters
  implement it: `RealFileSystem` via `fs_port::write_atomic_blocking` (sibling
  temp, target permissions carried over, rename over the target, temp dropped on
  failure), the in-memory double via the same two steps with an injectable
  commit-boundary failure. Every port caller inherits it — org write-back
  (`file_sync_controller.rs`), ingest normalization write-back, image
  materialization. The sync-base JSON sidecar (`sync_base_store.rs`) writes
  through the same helper. The temp is invisible to ingest on two independent
  counts (dot-prefixed → the `hidden(true)` walk skips it; non-`.org` → every
  org-relevance filter tests the extension), and a backend that pairs both
  rename sides now reads a from-outside-org-space rename as a `Create`, never as
  a document re-home.
  No `fsync` precedes the rename: on macOS that means `F_FULLFSYNC` on every
  write-back, and surviving power loss with the *newest* mirror bytes is not
  owed for an artifact re-derivable from the authority — what is owed (no reader
  ever sees an interior) comes from the rename alone.
  V5 discharged by inspection while auditing the callers: `SharedSnapshotStore`
  already writes every artifact it owns (snapshot, peers roster, port,
  generation) as tmp → `fsync` → rename. Still open: the write-outruns-authority
  ORDERING hole above is untouched by this — atomic application says nothing about *when* the mirror
  becomes durable relative to its authority commit.
- **Mint ordering:** already satisfied; guarded by the existing identity tests.
- **Order-mint atomicity:** SATISFIED for the SQL block authority. Minting a
  position is now a pure decision — `OrderKeyMinting::new_child_anchor` returns a
  `MintedPosition` (`holon-core/src/block_ordering.rs`): the `sort_key` plus the
  sibling re-keys the key is expressed against. Both halves travel to the write
  that consumes them and the SQL writer lifts the re-keys into the SAME
  transaction as the create, the batch, or the placement
  (`SqlOperationProvider::{create_row, place_row, apply_position}` and the batch
  `BatchOp.position`, `crates/holon/src/core/sql_operation_provider.rs`).
  Placement is one transaction over `parent_id`, `sort_key` and the re-keys, so a
  mid-failure can no longer leave a block re-parented under a key from its old
  parent's sequence.

  **Amendment (2026-08-09, Ruling B — typed re-key channel).** The re-keys
  originally travelled as the `_order_rekeys` *operation-control param*, packed
  into the caller-supplied `StorageEntity` by `MintedPosition::into_params` and
  read back by `SqlOperationProvider::order_rekey_statements`. Because the SQL
  writer INTERPRETED that map key, a peer- or MCP-supplied `_order_rekeys`
  property was a latent "rewrite any row's `sort_key`" primitive, defended only
  by filters at every data→params boundary (MCP, the Loro→SQL projection ×3,
  `BlockWriteField::parse`). Those filters compensated for a channel that should
  never have existed. The re-keys are now a TYPED field: `MintedPosition` travels
  whole (`into_parts`) to `create_row` / `place_row`, and per-op through
  `BatchOp { position: Option<MintedPosition> }`; the map codec
  (`into_params` / `decode_rekeys` / `order_rekey_statements`) is deleted. A data
  key literally named `_order_rekeys` is now inert — the writer never reads it, so
  it cannot reach the re-key channel (parse-don't-validate: the illegal state is
  unrepresentable). `prove_rekeys_are_siblings` survives as the in-process
  backstop against a MINTING bug and is called on `position.rekeys()` in all
  three typed sinks. The boundary filters and `is_operation_control_param` are
  KEPT unchanged as defense-in-depth (they no longer guard the *sole* protection).
  The Loro→SQL projection builds every `BatchOp` with `position: None` — it never
  mints re-keys — so an untrusted peer's `_order_rekeys` property is structurally
  unable to become a re-key, not merely filtered.

  The type still enforces both-halves-or-neither: `MintedPosition` is not `Clone`
  and its `sort_key` is reachable only by consuming it via `into_parts`, so a
  caller cannot spend the key and drop the re-keys.
  Regression tests (`sql_block_operations.rs`):
  `a_refused_create_leaves_no_sibling_rekey_behind` (red before: the re-keyed
  sibling read back `"7F80"` where the untouched keyspace says `"A0"`) and
  `a_refused_placement_leaves_neither_half_of_the_move_behind` (red before: the
  block sat under the NEW parent after the placement was refused); the typed
  channel is proven inert to a params-map `_order_rekeys` by
  `a_params_map_order_rekeys_key_is_inert_at_the_writer`, and the peer-projection
  path by the consolidator test naming a victim block.

## Consequences

- Non-block entities and table-displayed rows get birth atomicity for free: a SQL
  row born in one INSERT inside one transaction is a tier-complete firing; the rule
  writers must keep is "never split one logical birth across transactions".
- The saga/intent-log family is dropped entirely; crash recovery for mirrors is
  re-projection — which exists today only as forward propagation and boot ingest;
  the divergence-triggered repair loop is a named residual, not current code. The
  machinery that does exist is already load-bearing.
- The share-mount half-born case reclassifies as authority ambiguity: the on-disk
  marker (a mirror artifact) was treated as authoritative for mount existence while
  the registration was not. Its fix is a D5 declaration (which side owns mount
  existence), not new birth machinery.
- The E-style "born invisible, flip visible" mechanism is not adopted. It returns
  only if a genuinely multi-step birth within one authority ever appears; none is
  known.
- The descendant-withholding exclusion for projection readers stays safe: under D1
  there is nothing half-born to filter. Forward-looking only: vaults that caught the
  pre-lock window can still hold permanently withheld half-born subtrees; the
  authority-routed load-time backstop (1B′) remains unbuilt and is this ADR's named
  legacy residual.

## Open questions

- RESOLVED by ADR 0031: the per-type birth contracts live in the Holon-native
  transition catalog, which also brings the macro enforcement. This ADR is enforced
  manually until that catalog's guard gate lands.
- Ingest currently records file ownership AFTER accepting a file
  (`file_sync_controller.rs:2367`) and gates only on echo-suppression/hash — with
  D3.1's atomic writes this is acceptable (nothing torn can come from our writer;
  what remains on disk is user intent by definition), but the ownership-before-
  ingest question stays open alongside the reunification residual (#34).
- Facet-level authority declarations (D5) exist today as code behavior, not as a
  readable registry; whether to reify them into the ADR 0031 catalog is open — carried
  there as ruling P4, deferred.
- Validations owed (from the second review): V1 whether Loro mode performs a full
  boot-time Loro→SQL re-projection (would make SQL divergence self-healing — cite it,
  or the D3 ordering hole widens); V2 measure the actual per-op ordering of
  `save_doc` vs subscription-driven projection and org write-back (is the
  mirror-outruns-authority window real on the scheduler?); V3 — DONE, see the
  validation log below; V4 whether trust-gate proposal blocks leak into org
  write-back (D7 Open Decision 5 — if yes, a birth-adjacent mirror leak D3 must
  name); V5 atomicity audit of `SharedSnapshotStore` and roster sidecar writes —
  DONE, all four artifacts already write tmp → `fsync` → rename.

## Validation log

**V3 — crash-injection over the order re-key mid-loop. Answer: MIS-ORDERING, not a
benign later re-key — but only for some keyspaces, which is worse than either horn
of the question.**

Evidence (`crates/holon/src/core/sql_block_operations.rs`, two tests). The writer
applied its per-row `set_field("sort_key")` calls in sibling order, so the durable
residue of a crash is exactly a PREFIX of the plan; both tests replay prefixes
directly.

- `a_partially_applied_rekey_misorders_siblings` — siblings `alpha "7080"`,
  `beta "7080"` (the tie that fires the re-key), `gamma "7180"`. `gen_n_keys`
  spreads its output around the middle of the space, so the new keys land ABOVE
  these. ONE applied re-key (`alpha`) is enough: the parent reads back
  `beta, gamma, alpha`.
- `a_partially_applied_rekey_over_unkeyed_siblings_keeps_the_order` — the common
  shape (`"A0"`/`"A1"`, the unkeyed sentinels, which sort ABOVE every minted key)
  survives every prefix with its order intact.

So the cost is not "eventual re-key fixes it": a crash can leave the vault durably
mis-ordered, and whether it does depends on where the existing keys sit relative to
the generator's output — not on anything the writer controls or can test for. That
is the argument for folding the re-key into the firing transaction rather than
merely making the loop restartable, and it retires "partial application degrades
benignly" as a defence for any future keyspace rewrite.

## Alternatives rejected

- **Cross-store birth as an explicit PN subnet** (durable intent place + reaper
  transitions): modeled propagation inside the birth, coupling the PN contract to
  mirroring failures that re-projection already repairs; a full intent log
  duplicates what the authority already durably records. Rejected by Martin
  2026-08-08: creation needs observing only in the authority.
- **Read-side filtering per reader** ("settled reads"): N readers × M types of
  silent withholding; already rejected 2026-08-06 for tree readers.
- **Two-phase visibility as the general mechanism** (E): solves multi-step births
  that D1 makes non-existent; kept only as a future option.
- **Count-on-mirror birth checks** (verify the mirror before declaring born):
  inverts D2 and makes birth latency depend on projection lag.
