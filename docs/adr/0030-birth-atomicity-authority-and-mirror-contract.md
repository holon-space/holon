# ADR 0030 — Birth atomicity: transitions fire in the authority; mirrors converge under their own contract

Date: 2026-08-08. Status: accepted (Martin, after senior-review with codebase validation).
Companion to ADR 0029 (identity minting) and the D7 unified-transition design
(`~/.claude/plans/holon-45-agent-op-revert-design-2026-08-07.md`, pending ratification —
this ADR's contract is designed to fold into the D7 catalog when D7 lands).

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
operations are PN transitions; the enforcement point is the D7 reification machinery
(macros generate guard-then-fire, so an author cannot write a partial birth by
accident). Until D7 lands, the dispatcher's create arms carry the contract manually.

**D2 — Mirroring is orthogonal to birth.** A creation needs to be observed only in
the authority. Propagation to other representations (SQL projections in Loro mode,
org files, matviews) is the mirroring layer's concern; a mirroring failure is a
mirroring defect, never a violation of the birth transition's contract. There is no
cross-store birth saga: mirrors are (for store-owned facets — see D5) re-derivable
from the authority, so **re-projection is the repair mechanism** — the authority
itself is the durable record of what must exist, and no per-birth intent log is
needed.

**D3 — The mirroring contract.** Two guarantees keep half-born states from
re-entering through the mirror door:

1. **Per-authority-commit atomic application.** A mirror may lag; it must never show
   a between-commit state. Turso IVM's per-transaction deltas already give this for
   SQL-fed views. File mirrors have no transactions, so file write-back MUST use
   atomic replacement (write temp, rename) — see Enforcement; an in-place write that
   can tear is a contract violation, and a torn file is worse than a stale one
   because org files are also an ingest source: a torn mirror re-ingested becomes
   authority corruption.
2. **Divergence is disclosed and repaired by re-derivation, never persisted
   silently.** The existing store-wins stance and the file-sync ownership-proof
   machinery are instances of this.

**D4 — Two-authority births order their firings leak-safely.** Where one birth
touches two authorities (identity mint in SQL per ADR 0029, then structure in the
block authority), each firing is atomic in its own store and the crash window
between them must leak in the harmless direction: **mint first**. A minted-but-
unused id is garbage with no consequences — same-title re-mint is idempotent and
returns the same id (`bless_carried_same_normalized_title_is_idempotent`,
`identity_minting.rs:330`), so no title is ever stranded by a ghost mint. The
current create path already orders this way (`sql_operation_provider.rs:2717`
mints before the INSERT at `:2766`). The reverse leak — structure without minted
identity — is a defect, and reordering that way is forbidden.

**D5 — Authority is declared per (entity type, mode, facet).** Not per type alone:
the block authority flips between Loro and SQL by mode, and — decisively — org
files carry authored facets the store cannot represent (file-level property
drawers, non-Holon drawer keys, preserved verbatim per
`file_level_property_drawer.rs`). For those facets the FILE is the authority and
the store is not involved. Consequently repair-by-re-derivation (D2/D3.2) is scoped:
it may regenerate only store-owned facets and must preserve file-authoritative
ones. A "mirror" is a mirror only facet-wise; blind full-file regeneration from the
store is a data-loss bug, not a repair.

## Enforcement

- **Guard-then-fire:** D7 macros once ratified; until then, create arms validate
  the full birth contract before the first write (the marks F3 fix is the
  pattern). Keystone invariant target: no observer ever sees an entity that fails
  its type's birth contract.
- **Fault-injection PBT:** inject failures between mint and create, and between
  authority commit and mirror application; the only observable end states are
  "never existed" and "fully born (mirrors eventually converged)". This is the
  checkable form of "no permanent loss".
- **Atomic file replacement:** `FsPort::write` in-place write
  (`fs_port.rs:83`, used by `file_sync_controller.rs:6038`) must become
  temp+rename. Tracked as its own remediation task; until it lands, D3.1 is
  known-unsatisfied for file mirrors and this ADR's row in the tracking docs says
  so.
- **Mint ordering:** already satisfied; guarded by the existing identity tests.

## Consequences

- Non-block entities and table-displayed rows get birth atomicity for free: a SQL
  row born in one INSERT inside one transaction is a tier-complete firing; the rule
  writers must keep is "never split one logical birth across transactions".
- The saga/intent-log family is dropped entirely; crash recovery for mirrors is
  re-projection, which already exists. Less machinery, and the machinery that
  remains is already load-bearing.
- The share-mount half-born case reclassifies as authority ambiguity: the on-disk
  marker (a mirror artifact) was treated as authoritative for mount existence while
  the registration was not. Its fix is a D5 declaration (which side owns mount
  existence), not new birth machinery.
- The E-style "born invisible, flip visible" mechanism is not adopted. It returns
  only if a genuinely multi-step birth within one authority ever appears; none is
  known.
- The descendant-withholding exclusion for projection readers stays safe: under D1
  there is nothing half-born to filter.

## Open questions

- D7 ratification decides where the per-type birth contracts live concretely (the
  transition catalog) and brings the macro enforcement; this ADR stands without it
  but is enforced manually until then.
- Ingest currently records file ownership AFTER accepting a file
  (`file_sync_controller.rs:2367`) and gates only on echo-suppression/hash — with
  D3.1's atomic writes this is acceptable (nothing torn can come from our writer;
  what remains on disk is user intent by definition), but the ownership-before-
  ingest question stays open alongside the reunification residual (#34).
- Facet-level authority declarations (D5) exist today as code behavior, not as a
  readable registry; whether to reify them (likely into the D7 catalog) is open.

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
