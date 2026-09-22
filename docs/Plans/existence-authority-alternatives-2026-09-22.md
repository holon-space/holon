# Block existence: one authority, or peers that converge?

*Design study, 2026-09-22. Top-down, from requirements; deliberately not
derived from the current code. Companion to
[never-seeded-adoption-2026-09-22.md](never-seeded-adoption-2026-09-22.md)
(the D175.a plan, "approach A" below). Terms follow
[Model.md](../Architecture/Model.md).*

The question: instead of one existence authority (Loro), treat every store as
a peer, write to a leader by default, let data flow leader → others, and also
sync data a non-leader has that the leader lacks. Is that possible at all under
the requirements? Where does it break? What lies between the two?

---

## 1. Requirements and constraints

### 1.1 The three stores, as facts

| Store | Holds | History | Tombstones | Written by | Edited from outside |
|---|---|---|---|---|---|
| **Loro** | tree + text + most fields | full op log, frontiers, version vectors | yes, until GC | app ops, peer sync | no |
| **Turso** | every field incl. SQL-only ones | none | none unless added | one projection writer | no |
| **Org** | text-representable subset; lossy | none (mtime, hash) | none | app write-back | **yes**: editors, git, sync tools |

Only Loro crosses devices. Org files are per device (invariant 11: a byte-level
file syncer on the vault is out of contract). Invariant 11 is derived, not
asserted: by §1.3 a delete can only be asserted by a store holding a base, a
tombstone or a history, and a byte-level syncer holds none — it moves bytes,
not evidence. So it fails on its own, with or without Loro beside it: it
resurrects a deleted block by delivering an older copy of the file (the file
diff reads the block as a create), it resolves two-device edits by file-level
last-write-wins with no disclosure, and it copies a never-seeded block to both
devices so each adopts it. "Only one system syncs" is the corollary: cross-
device transport goes through the consolidator's history, and a syncer that
carried the per-file base and tombstones with the file would itself be a
consolidator, which is R6 again. In SqlOnly mode nothing crosses devices, so
that vault is single-device by construction (open question 5).

### 1.2 Requirements (numbered; the table in §5 uses these)

- **R1 Non-blocking.** The UI never waits on reconciliation. Boot draws
  incrementally; any repair runs beside normal operation and is disclosed if
  it outlives the initial-scan window (invariant 14 ranks silent degradation
  last).
- **R2 External edits are intent.** A change to an org file made by any tool
  is a user edit and must reach the other stores (invariant 1: a replica's
  inbound intent is `diff(base, current)`).
- **R3 No delete amplification.** A crash, a half-written file, a truncated
  file or a partial projection must never be read as "the user deleted these
  blocks" and then propagated. A delete needs positive evidence.
- **R4 Identity minted once.** A block ID is minted at first entry and never
  re-minted (invariant 13). Every reconciliation resolves-before-minting.
- **R5 Forks are loud.** Two nodes carrying one stable ID, or two stores
  disagreeing on a field with no merge rule, must surface as an error or a
  condition, never as a silent last-writer-wins.
- **R6 One consolidator per epoch.** Bases are only meaningful against one
  consolidator's linear history (invariant 10). Changing who merges is a
  migration, never a runtime lookup.
- **R7 Sinks never re-merge; one writer per store** (invariants 4, 5).
- **R8 Tombstones outlive every base** (invariant 9), else a stale replica
  resurrects a deleted block.
- **R9 Field classes.** Measured against the schema (scout, 2026-09-22):
  no AUTHORED field lives in SQL only. The fields not present in all three
  stores fall into three classes: DERIVED (`sort_key`, `property_kinds`:
  recomputed, never reconciled), CONTROL (`collapsed`, `widget_only`,
  `created_at`, `updated_at`, `write_seq`, `_change_origin`: local
  bookkeeping, Loro+SQL, never on disk, never reconciled), and AUTHORED
  fields Org cannot carry (the edges `requires`, `contributes_to`,
  `advice_suppressed`: Loro+SQL, no org syntax). "Agreement" is therefore
  defined on authored fields only, and Org's subset is "authored minus edges".
- **R10 Pre-Loro vaults migrate without a wipe.** Rows that predate the Loro
  store must become Loro nodes with their IDs intact. Because no authored
  field is SQL-only (R9), adoption from the org file loses nothing: every
  authored field of such a row came from the file and is still in it.
- **R11 Keystone-testable.** The reference model must be able to state the
  expected outcome of every transition, including crash and reboot, without
  re-implementing the SUT's merge.

### 1.3 The one theorem that shapes everything

Given two **current-state** snapshots `S_A`, `S_B` and an ID `X` with
`X ∈ S_B`, `X ∉ S_A`, two histories fit the evidence:

- (h1) `X` reached both; A deleted it; the delete has not reached B.
- (h2) `X` reached B; it never reached A.

Nothing in the two snapshots distinguishes them. Resolving needs one of:

- a **base** for the pair (the last state both agreed on: `X ∈ base` → h1,
  `X ∉ base` → h2),
- a **tombstone** in A (A remembers deleting `X`), or
- a **history** in A (A can answer "have I ever seen `X`?").

These are the same fact in three encodings. Every design below is a choice of
which stores carry which encoding. A store with none of the three can feed
creates but can never assert a delete against another store.

---

## 2. Approach A — single existence authority

**Rule.** A block exists iff Loro's tree holds a node for it. Turso is a
derived index; Org files are replicas with a per-file base. Every store may
feed **creates**: a row or a file block that Loro lacks is adopted, ID
preserved, through the ordinary new-block path. **Deletes** flow only from
Loro (app op, or a peer's merged op) or from a store that holds a base against
Loro (an org file: `X ∈ file base ∧ X ∉ file now` is an external delete).

**Strongest form.** Adoption is the file-change path, not a migration: a scan
adopts as it goes, the UI sees each node the moment it exists (R1). Turso never
originates a delete, so nothing in Turso can amplify one (R3 for Turso). The
org file base is the one place external deletes are decided (R2, R3 for Org).
One adopter per block (the peer holding its home file), no adoption while a
mount or pairing is live, and a duplicate stable ID is an unconditional error
(R4, R5). SqlOnly mode is the same shape with Turso as consolidator (R6).

**Where pure A is exposed.** Between a Loro delete op and its projection into
Turso, a crash leaves a row Loro no longer has. On the next boot A's rule reads
that row as a create candidate and resurrects the deleted block. Pure A has no
base for the Loro→Turso pair, so it cannot tell that row from a never-seeded
one. §4 (C1, C2) closes this; the plan's ordering ("project first, adopt
after") only works if the projection itself is a base diff.

---

## 3. Approach B — peers converge, leader by availability

**Rule.** Every store is a peer. The leader is Loro if available, else Turso,
else Org. Writes go to the leader; data flows leader → others; data a
non-leader holds and the leader lacks flows back.

### 3.1 What "agree" can mean

Per R9, agreement is on authored fields, per store on the subset it carries.
Derived fields are recomputed, control fields are local, so neither enters
the question. Two clauses remain: every store that carries an authored field
holds the same value; and the three stores hold the same set of IDs. The
second clause is the whole problem: one bit per ID, and §1.3 says two
current-state stores cannot settle it.

Field agreement also shows the leader-by-store rule is ill-formed at the
bottom of the chain: Org cannot lead the edge fields it cannot carry. A
leader chain that ends in Org must become leader-by-field (C3) or give the
edges an org representation.

### 3.2 A base per store pair

Loro-mode has three pairs. For each, §1.3's evidence:

| Pair | Direction | Evidence available | Verdict |
|---|---|---|---|
| Loro ↔ Turso | Loro deleted, Turso has row | Loro history + tombstones | resolvable |
| | Turso has row Loro never saw | Loro history ("ever seen?") | resolvable |
| Loro ↔ Org | Loro deleted, file still has block | needs the **file base**; Loro history alone is ambiguous because an editor may have re-added the block on purpose | resolvable only with a file base |
| | file deleted block, Loro has node | file base | resolvable only with a file base |
| Turso ↔ Org | any | neither has history; needs a file base **and** Turso tombstones | resolvable only with both added |

Two facts fall out. First, Turso never originates a delete or a create except
the one-time pre-Loro rows; its peer-ness is nominal. Second, every resolvable
cell uses either Loro's history or a base — the same two encodings A uses.
The Turso↔Org pair in Loro-mode should not exist at all: resolving it directly
makes a second writer for Turso (R7). Both stores are downstream of Loro.

### 3.3 Failure modes, one by one

Each is marked **fatal** (B cannot meet a requirement), **repairable** (a rule
inside B fixes it), or **constraint** (B must adopt it; noted when adopting it
makes B indistinguishable from A).

1. **Deletion from snapshots** (§1.3). Fatal without evidence. Constraint:
   every pair carries a base, or the leader carries history and the other side
   never originates deletes. That constraint is A's rule word for word.
2. **Crash between Loro write and projection.** Turso lacks `X`. Repairable:
   Turso is re-projected from a base diff. That requires Turso to record the
   Loro version it last projected (C2). Same repair A needs.
3. **Crash between Loro delete and projection.** Turso has a row Loro lacks.
   Without a base B adopts it back — a resurrection, violating R3. Repairable
   only via Loro history (C1) or a Turso base (C2). Same as A's exposure.
4. **File truncation / half-written file.** Base has N blocks, file has 0 or
   k < N. The file diff reads as N−k external deletes. Fatal to naive B and
   equally to A: the org replica contract itself says a missing block is a
   delete. Constraint, shared: a delete-amplitude guard (a file that loses
   more than a threshold of its blocks in one edit is quarantined with a
   disclosed condition until confirmed) and a parse failure yields *no* diff,
   never an empty one. Loro-side deletes are reversible from history; Turso
   and Org deletes are not — a further reason the guard sits before the
   leader is written.
5. **Concurrent app edit and external edit of one block.** B with no base has
   no merge, only a winner by mtime — R2 and R5 fail. With a file base it is
   the ordinary degradation ladder (op-CRDT ≻ 3-way ≻ LWW with disclosure).
   Constraint; adopting it is A.
6. **Leader by availability.** Device boots with the Loro store unreadable →
   Turso leads → fractional indexes minted in Turso's keyspace, bases
   advanced against Turso's history → Loro returns → two consolidators'
   histories are incomparable (R6, invariant 10: phantom diffs, fake
   conflicts). **Fatal** as stated. Constraint: leadership is a vault-level
   epoch fixed at migration, and "Loro unavailable" is a refusal to boot into
   write mode, not a fallback. Adopting it removes the leader chain and B's
   leader is A's authority.
7. **Multi-device adoption.** Two devices each hold a never-seeded `X`
   (pre-Loro vault copied to both, or a stale file on one). Each adopts →
   two TreeIDs, one stable ID. Fatal to R5 unless one adopter is elected. B
   makes every non-leader store on every device an adopter, so the fork
   surface is devices × stores. Constraint: single adopter (home-file
   holder), duplicate stable ID is an unconditional error. Same rule A
   needs; B has more sites to apply it.
8. **Tombstone GC** (R8). A peer whose base predates a tombstone resurrects
   on its next diff. Repairable inside B only if every peer, including Turso,
   registers a base that the GC respects. That is C2.

### 3.4 What survives of B

B's honest strongest form is: creates flow from every store (A already says
this); deletes flow only where evidence exists (A's rule); every store carries
a base (A's invariant 1 extended to Turso, which is C2); leadership is an
epoch (A). The only element of B that does not reduce to A is the availability
fallback, and that element is the one that violates invariant 10. B is A with
a hazardous leader-election rule attached. The merit of the exercise is that
it exposes where A is under-specified: the Loro↔Turso base.

### 3.5 The four objections, as hypotheses

| Hypothesis | Verdict |
|---|---|
| Asymmetric deletes make B impossible | **Confirmed** as a theorem (§1.3), not as a B-specific flaw: it forces both A and B to hold evidence. |
| B needs lossless projections | **Refuted.** Per-field agreement on the carried subset suffices. What it does force: a lossy store cannot lead, so leader-by-store must become leader-by-field at the chain's end. |
| B cannot pick a concurrent-edit winner | **Refuted** as B-specific. With a base it merges like A; without a base it has no merge at all — that is the fatal form, and the fix is the base. |
| B forks under multi-device | **Confirmed**, and stronger than stated: the availability fallback alone breaks the epoch invariant even with one device. |

---

## 4. Approach C — hybrids

### C1 — Loro is the delete authority; absence is judged against history

"Absent from Loro" is asked of Loro's history, not its current tree:
never-seen → adopt; seen-and-deleted → the row is stale, drop it.

- **Buys:** closes A's resurrection hole (§2, §3.3 item 3) with one query per
  candidate and no new persisted state. Pre-Loro rows are, by definition,
  never-seen.
- **Costs:** correct for Turso only. For Org, seen-and-deleted is ambiguous
  (an editor may re-add a block on purpose), so Org still needs its file base.
  Depends on tombstones being retained (R8): after GC, "never seen" and
  "seen-and-forgotten" collapse. Needs a boot-time full comparison of Turso
  against Loro (O(N), off the render path) unless combined with C2.

### C2 — per-pair bases; Turso records the Loro version it last projected

Turso becomes a well-formed replica under invariant 1: one base column (the
Loro frontier at the last complete projection), advanced in the same
transaction as the projected rows.

- **Buys:** boot projection is a Loro diff from the base (O(delta), which is
  the D172.a read-model shape anyway); a crash before the base advances
  simply replays; deletes reach Turso without any Turso→Loro question ever
  being asked. Turso registers as a base for tombstone GC (R8). After the
  diff replays, any Turso row Loro lacks is exactly a never-seeded row; a
  pre-Loro vault has an "epoch zero" base, so all its rows are candidates.
- **Costs:** one column and one migration boot that runs a full projection in
  the background (R1 holds: rows appear as they project). The base must be
  written transactionally with the rows, or C2 is worse than nothing.

C1 and C2 overlap: with C2 in place C1 is no longer a decision rule, but it
remains the cheapest **assert** — an adopter must never adopt an ID Loro has
ever seen — which pins C2's transactional guarantee.

### C3 — leader by field, not by store

With the R9 measurement, C3 is a short table rather than a mechanism:

| Class | Fields | Authority | Syncs? |
|---|---|---|---|
| authored, text-representable | content, id, task_state, priority, tags, scheduled/deadline, content_type, source_*, marks, properties | consolidator (Loro); Org replica with a file base | yes |
| authored, no org syntax | edges: requires, contributes_to, advice_suppressed | consolidator (Loro) | yes; invisible to external editors until org gets an edge syntax |
| structural | parent_id (implied by nesting in org) | consolidator | yes |
| derived | sort_key, property_kinds | recomputed from the consolidator | no — recomputed |
| control | collapsed, widget_only, created_at, updated_at, write_seq, _change_origin | Turso (local bookkeeping) | no — local |

- **Buys:** names what invariant 12 already does. Makes "agree" well-defined
  for R9 and removes B's chain-end problem. Says out loud that control fields
  never sync, which is true today and undocumented.
- **Costs:** one more table to keep total when a field is added. The only
  real gap it exposes is the edge fields: authored data that Org cannot carry
  is the single place where an external editor sees less than the app.
  Existence stays with the consolidator.

---

## 5. Comparison

| Requirement | A | B | C1 | C2 | C3 |
|---|---|---|---|---|---|
| R1 non-blocking | yes, adopt-as-scan | yes, if per-pair repair is background | yes | yes, diff replay is O(delta) | n/a |
| R2 external edits | file base | file base (must add) | unchanged from A | unchanged | unchanged |
| R3 no amplification | Turso safe; Org needs guard | Org and Turso both need evidence + guard | closes Turso resurrection | closes it transactionally | n/a |
| R4 identity once | adopt preserves ID | same, more adopters | same | same | same |
| R5 forks loud | single adopter + dup error | devices × stores adopters | same as A | same as A | n/a |
| R6 one epoch | yes | **violated** by availability fallback | yes | yes | yes |
| R7 one writer | yes | Turso↔Org pair breaks it | yes | yes | yes |
| R8 tombstone GC | Turso unregistered | every peer must register | depends on retention | Turso registered | n/a |
| R9 field classes (authored only; edges have no org syntax) | implicit | forces per-field leader at the Org end | unchanged | unchanged | explicit: the C3 table |
| R10 pre-Loro vault | adopt never-seeded | adopt, on every device | never-seen = candidate | epoch-zero base | n/a |
| R11 keystone | reboot + adopt transitions | model must implement merge = SUT | one history-query oracle | base column is observable | field table is a fixture |
| Complexity | low | high (3 pairs, tombstones, leader) | +1 query | +1 column, txn | +1 table |
| Migration of a vault | none beyond adopt | bases for 3 pairs, Turso tombstones | none | one background full projection | none |
| Paired second device | single adopter rule | fork surface multiplied | same as A | same as A | SQL-only fields still local |
| Crash Loro-delete → projection | **resurrects** | resurrects | safe | safe | n/a |

**Verdicts.** A: correct in shape, one hole (resurrection after crash).
B: not viable as stated (R6, R7); every viable part is A. C1: closes A's hole
cheaply, Turso only, retention-dependent. C2: closes the same hole
structurally and makes Turso a proper replica; the strongest single addition.
C3: a naming of existing behaviour, worth writing down, not a mechanism.

---

## 6. Recommendation

Adopt **A + C2, with C1 as an assertion**. The decisive trade-off: B's promise
is that no store is special, but §1.3 shows a delete can only be asserted by a
store holding a base, a tombstone or a history — and giving each store that
evidence is exactly what makes Loro the authority and the others replicas with
bases. What B does add (leader by availability) is the one thing invariant 10
forbids. C2 is the piece A is missing: without a Loro→Turso base, pure A
resurrects a block deleted just before a crash, and its boot projection is a
full comparison instead of a diff. C1 costs one query and turns that
guarantee into a loud failure if the transaction is ever broken. C3 should be
written into Model.md as a field-authority table; it changes no code.

### Open questions for Martin — RULED 2026-09-22

1. **Delete-amplitude guard → HONOUR, visible and recoverable; strategy
   pattern.** An external edit that removes many blocks is applied, disclosed
   as a condition, and revertible from the consolidator's history (a Loro
   delete is reversible from the op log). Two strategies behind one policy
   point: `Honour` (default: apply, disclose, offer revert) and `Quarantine`
   (hold as a pending proposal until confirmed — reuses the confirm-a-proposal
   surface AI proposals already use). Two rules stay unconditional under both:
   a file that does not parse yields no diff, and the guard runs before the
   consolidator is written.
2. **Resurrection through a file → RESURRECT when it is certainly not a lost
   delete.** The file base decides: `X ∉ base ∧ X ∈ file` means the editor
   re-added X after the delete reached the file → reuse the stable ID on a new
   node. `X ∈ base ∧ X ∈ file` means the delete never reached this replica →
   the file is stale and the delete wins. `ever_seen = Deleted(_)` alone does
   not decide; the base does.
3. **Turso as a registered base for tombstone GC → RATIFIED.**
4. **Single adopter = home-file holder; no adoption under mount or pairing →
   RATIFIED** for every approach (this closes D178's peer half).
5. **Epochs → build the abstraction, not the second consolidator.** Write
   `Consolidator` and `TextFormat` (§7, §7.1) now; implementations: Loro and
   the keystone model. `GitConsolidator<F: TextFormat>` is declared, not
   implemented. (This is also the answer to question 6 below: yes.)

**Still open (one): does SqlOnly stay a live mode?** Martin's proposal
(2026-09-22 15:21): **Text+VCS is the second consolidator; Turso is not yet
one.** Turso does not qualify today because it holds no retention-bounded
delete history and its Sync merges by silent last-push-wins; future Turso
enhancements (a retained CDC log with a bound, a surfaced conflict, a
self-hosted or peer hub) could change that, and the trait is the seam where
a Turso impl would then plug in. If Loro or Text+VCS is the vault's epoch, it consolidates; a
Turso-only vault does not consolidate at all. Two conditions make that
sound, both already in this document:

1. "Available" means the vault's *configured epoch*, fixed at migration and
   checked by `epoch_id` (R6, §7) — never what happens to be readable at
   boot. A git-backed vault has no Loro store, so there is nothing to fall
   back to; the choice is per vault.
2. A Turso-only vault is consolidation-free only if there is no replica to
   reconcile against. §1.3 does not care about sharing: the moment an org
   file can be edited from outside (R2) or truncated (R3), some store must
   answer "was X here before?", and Turso holds none of the three encodings.
   So in a Turso-only vault the org files are **import-only** (§7.2: feed
   creates, never assert deletes) or an export the app does not read back.
   With that restriction Turso is a single-writer index with nothing to
   consolidate.

What follows, for now: SqlOnly as a live *reconciling* mode goes away; Turso
needs no tombstones; Turso's C2 base column is one `Version` in every epoch (a
frontier or a commit hash); a vault without Loro is a
`GitConsolidator<OrgFormat>` vault once that impl exists; until then every
live vault is Loro-mode and a pre-Loro vault is an import path (R10).
Confirmation → D179.d.

**Turso Sync, checked against the docs (2026-09-22).** Martin asked whether
Turso's own Sync feature changes this. Findings, with sources:
- Delete evidence: real. The new engine's CDC log (`turso_cdc`, `change_type
  -1/0/1`, before/after state) is a persisted, queryable table on the client —
  docs.turso.tech/tursodb/cdc.
- Retention: not documented; "query, maintain, and clean up with standard
  SQL" — no bound, no offline-past-retention behaviour described (R8
  unanswerable).
- Merge rule: *"row-level logical logging with a Last-Push-Wins conflict
  resolution strategy … the version that is pushed last will take precedence
  — regardless of the local commit time"*, resolved silently unless the app
  supplies a `transform` hook — docs.turso.tech/sync/usage. That is the bottom
  rung of the ladder and violates R5 as shipped.
- Hub: *"The remote database acts as the source of truth"*; *"fully
  distributed, peer-to-peer synchronization between devices is not supported"*
  — Turso Cloud required.
- Product: Sync is the libSQL-flavoured offering; no page states it exists for
  the Rust engine Holon pins (tursodatabase/turso). CDC does exist there.

Impact: none on the ruling. In a Loro or git epoch, Turso Sync must stay off
(a second cross-device history in one epoch is exactly what R6 forbids). In a
Turso-only vault it would make Turso a consolidator with silent LWW and a
hosted hub — so Turso is not yet a consolidator and D179.d stands as
worded: not yet, revisited when Turso's sync semantics change. One thing it does give: `turso_cdc` is a
ready-made tombstone source should a Turso `ever_seen` ever be wanted; it is
not wanted under D179.d.

---

## 7. The interface: a `Consolidator` trait

§1.3 makes the interface small. A consolidator is exactly a store that answers
the three encodings for a pair; everything above it in this document is
generic over that and never names Loro. Sketch, in the document's own terms:

```rust
trait Consolidator {
    /// Loro: a frontier. Git: a commit hash. A partial order is enough;
    /// no base check needs a total one.
    type Version;
    fn is_ancestor(&self, a: &Self::Version, b: &Self::Version) -> bool;

    /// Fixed at migration, checked at boot (R6). A mismatch is a refusal,
    /// never a fallback.
    fn epoch_id(&self) -> EpochId;
    fn head(&self) -> Self::Version;

    /// Per entity (any kind): create / update(fields) / delete, restricted to the fields
    /// this store carries (R9). The one thing C2 needs: Turso's base column
    /// is a Version and boot is `diff(base, head)`.
    fn diff(&self, from: &Self::Version, to: &Self::Version) -> Delta;

    /// C1 as a method. Loro answers from its op log, git from
    /// `log -S` on the `:ID:` property; Turso-as-consolidator cannot answer
    /// without added tombstones — the trait makes that gap explicit.
    fn ever_seen(&self, id: &EntityUri) -> Seen; // Never | Live | Deleted(Version)

    /// The store declares its ladder rung (op-CRDT, 3-way, none). A
    /// Conflict is a value the caller can disclose (R5), never a silent
    /// winner.
    fn merge(&self, base: &Self::Version, ours: &Self::Version, theirs: &Self::Version)
        -> Merged<Self::Version> /* | Conflict */;

    /// The GC contract for R8. Loro: retention. Git: a no-op, history is
    /// never dropped.
    fn register_base(&mut self, replica: ReplicaId, at: Self::Version);
    fn release_base(&mut self, replica: ReplicaId);

    /// The single writer (R7).
    fn apply(&mut self, ops: Ops) -> Self::Version;
}

trait Replica<C: Consolidator> {
    fn base(&self) -> C::Version;
    fn current(&self) -> Snapshot;
    fn carries(&self, field: FieldId) -> bool;
    // Inbound intent is diff(base, current) — invariant 1 verbatim.
}
```

**Entity, not `Block`.** Every `Block` above means *entity*: anything with a
stable `EntityUri` (the `block:` / `doc:` schemes today, more kinds coming).
The traits are generic over the kind: `Delta` is per entity, `ever_seen` takes
an `EntityUri`, `carries(kind, field)` is per kind, and a `TextFormat` parses
a file into entities of several kinds (an org file yields one `doc:` and many
`block:`; a cooklang file might yield a recipe, its steps and its ingredient
lines under kinds of their own). The existence rule, adoption and the
single-adopter rule are stated once for `EntityUri` and never per kind. The
one place kind matters is the delta lift in §7.1: `line_span` and the
resolve-before-minting step are per entity, and containment between kinds
(a `block:` inside a `doc:`) is the tree the format's parser returns, not a
property of the trait.

Above the trait, and never naming a store: the existence rule, adoption, the
single-adopter rule, the delete-amplitude guard, the duplicate-ID error. The
keystone model (R11) gets an implementation with a trivial linear history,
which is the strongest argument for writing the trait at all: three real
implementations (Loro, Org+VCS, the keystone model), not a hypothetical.

Where it strains — the places the "hard-coding Loro" worry is real:

1. **Delta granularity.** Loro's diff is block-level; git's is line-level, so
   a moved block reads as delete plus create. The git implementation must lift
   to the block-level `Delta` itself, and can only do so with the
   resolve-before-minting step inside the impl (a create whose stable ID
   matches a `Deleted(_)` in the same diff is a move). A real piece of logic
   per impl, not a thin adapter.
2. **Merge initiative.** Loro merges on sync, continuously; git merges on
   pull, on the user's initiative, with a human resolving. The trait has no
   "sync now" method; convergence is a property the impl states (continuous
   vs on-commit), and R1 is met differently under each.
3. **Writer concurrency.** `apply` on Loro may run beside an incoming peer
   merge; on git it must not run while a merge is in progress or a conflicted
   file is on disk. The "parse as no diff" rule (§3.3 item 4) is generic; the
   condition that triggers it is store-specific.

Open question 6 (RULED yes, see §6 item 5): **write the trait now, with the keystone
model as its first implementation.** It costs a refactor of the Loro seam the
D175.a plan touches anyway, and it decides open question 5 by making the
Org+VCS epoch a second impl instead of a fork.

### 7.1 Generalising Org+VCS to Text+VCS: what the format and parser must provide

Org is not special in §7; the git half of Org+VCS is generic. What is
format-specific is the lift from git's line-level evidence to block-level
`Delta`, and that lift is possible only if the text format and its parser
satisfy a small contract. Stated as a trait so Obsidian-Markdown, cooklang or
any future format is a second implementation rather than a fork:

```rust
trait TextFormat {
    /// Bytes → blocks, deterministic, total over the format (a file that
    /// does not parse is a refusal with a location, never a partial tree).
    fn parse(&self, bytes: &[u8]) -> Result<Tree, ParseError>;
    /// Blocks → bytes. Round-trip law: render(parse(b)) == b for every
    /// file the app has not changed, byte for byte. Without this every
    /// write-back is an external edit and the base diff sees phantom
    /// changes (R2 becomes noise, R3 is at risk).
    fn render(&self, tree: &Tree) -> Vec<u8>;
    /// The stable ID of an entity, read from the bytes. `None` means the
    /// format cannot carry one for this entity.
    fn entity_id(&self, entity: &Entity) -> Option<EntityUri>;
    /// Write a minted ID into the block's bytes on first ingest, so the ID
    /// is in git history from then on (R4, and `ever_seen` via `log -S`).
    /// `Err(Unsupported)` demotes the format to import-only (below).
    fn stamp_id(&self, entity: &mut Entity, id: EntityUri) -> Result<(), Unsupported>;
    /// Which fields the format can carry (R9); the rest are SQL-only and
    /// never travel through this replica.
    fn carries(&self, kind: EntityKind, field: FieldId) -> bool;
    /// A file holding VCS conflict markers parses as "no diff", never as
    /// N deletes (§3.3 item 4).
    fn is_conflicted(&self, bytes: &[u8]) -> bool;
}
```

Consequences, per format:

- **Org (today).** Satisfies all of it: `:ID:` in the drawer, byte-stable
  round trip pinned by `org_store_org_round_trip.rs`, headline = line span,
  `carries` is the documented text-representable subset.
- **Obsidian Markdown.** `block_id` exists only for blocks the user marked
  with `^id`; every other block has none. `stamp_id` is possible (append
  `^id`), which changes the user's files on first ingest — a policy question,
  not a technical one. Line spans are well defined for headings and
  paragraphs; list items need the parser to decide the granularity. Round
  trip is the hard requirement: most Markdown parsers are not byte-stable,
  and a lossy renderer disqualifies the format as a replica (it may still be
  an import source).
- **cooklang.** No identity in the format and no natural place to stamp one
  without changing the file's meaning; block granularity (recipe, step,
  ingredient line) is a modelling choice. Likely `stamp_id → Unsupported`:
  import-only.

### 7.2 How to get an entity delta from a file change

A file changed from version `a` to version `b`. We need an entity-level
`Delta`: which entities were created, changed, moved or deleted. Two ways:

1. **Hunk attribution.** Git gives line hunks. Map each hunk to the entity
   that owns those lines.
2. **Parse-and-diff.** Parse the old bytes and the new bytes. Key both
   results by `EntityUri`. Diff the two sets.

| | Hunk attribution | Parse-and-diff |
|---|---|---|
| Correctness | Fragile. A reformat or a moved drawer points a hunk at the wrong entity. A hunk that spans two entities needs a rule. A move looks like delete + create. | Exact. The diff runs on the same structures the app uses. A move is one ID at two positions. |
| What the format must provide | `line_span` per entity, no shared lines, a rule for spanning hunks | only `parse` and `entity_id` |
| Cost | proportional to the hunk | two parses of the changed file; only files git reports as changed |
| Old bytes | not needed | `git show a:path` (cheap). The old bytes must parse with the current parser. |
| Shared with the file watcher? | No. The watcher has no hunks. | Yes. The watcher already compares old bytes with new bytes. One code path. |
| Entities without an ID | not matched | not matched (`stamp_id`, or import-only) |
| Conflict markers | a hunk reads as N deletes unless guarded | `parse` refuses the file (`is_conflicted`). The delta is empty. One guard. |

**Choice: parse-and-diff.** The org ingest already works this way: a
three-way diff of parsed blocks against the file's base. So the git
consolidator does not add a second diff engine. Git then does three things,
and each is already a `Consolidator` method: give the old bytes for the diff,
answer `ever_seen` with `log -S` on the ID, and `merge`. `line_span` is
removed from `TextFormat`. The cost is one parse per changed file, twice.
The file watcher already pays that cost.

Two new requirements follow:

- **Parser versions.** Bytes committed under an older format revision must
  parse with the current parser. If they cannot, `parse` must accept a version
  hint: a version marker in the file, or the commit date.
- **Large files.** If two parses per change cost too much, the file is too
  large. Split the file. Do not fall back to hunks.

**Import-only** is the demotion path and it is honest: a format without
identity in its bytes can feed creates (mint on ingest, keep the mapping
file→ID in the consolidator) but can never assert a delete and never carries
history, so it is a source, not a replica and not a consolidator. The
`Consolidator` for Text+VCS is then generic over `TextFormat`:
`GitConsolidator<F: TextFormat>`, and "Org+VCS" is `GitConsolidator<OrgFormat>`.

Nothing above needs to be implemented now. The point is the seam: the
existence rule and adoption talk to `Consolidator`; `Consolidator` for git
talks to `TextFormat`; no code above those two traits names Org, Loro or git.
