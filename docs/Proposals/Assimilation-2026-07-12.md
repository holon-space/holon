# Assimilation — external items becoming native blocks (2026-07-12)

**Status:** RATIFIED 2026-07-13 (Martin ruled R1–R4, see §9). C5 is woven and its
coerced-proposal-emission wording is ratified; G1 (identity mapping + fixture-based
directed test, §10) is the first implementation increment.

**Rulings (2026-07-13):** R1 = `assimilated_from` round-trips through org. R2 =
candidates are a maintained view only, no pre-minted proposal blocks (replica tables
stay directly SQL-queryable and usable via reference blocks rendering live from the
replica row). R3 = default disposition `complete`. R4 = C5-D1 multi-op payload as a
follow-up after the C5 weave. Field mapping (status/due/priority etc.) is §5's
`assimilation.template` sidecar section — G1's directed test must assert those fields.

**Driving workflow (Martin):** the Todoist Inbox is the universal capture point (links,
ideas, anything). During triage each item takes one of three exits:
1. **stays external** — moved to another Todoist project (Todoist remains the system of record);
2. **gets ASSIMILATED** — becomes a native Holon project/block/task, managed in Holon from then on;
3. **(new, falls out of this design) gets LINKED** — referenced from Holon, but Todoist
   remains authoritative.

**Relates to:** ADR 0024 (P4 effect taxonomy, deterministic effect ids, leases; place
kinds — canonical/ratcheted vs display/maintained), ADR 0025 (ops are the only
propagation currency; connectors emit ops, never mutate state behind the projection),
Model.md Layer 1 (external replicas; "no replica writes another replica"),
VisionGapAnalysis C1/C5 (the Integrator's core loop = candidate generation +
confirmation queue — assimilation IS that loop), C2a/C2b (landed:
`ProvenanceStamp`/`_provenance`, `block_history`), the C5 trust-gate stream
(`ProposalRecord`, `_proposal`, `accept_proposal`), and
`docs/Proposals/Templating-2026-07-12.md` (block-shape templates).

## 0. First principles

- **Assimilation is authority transfer, not data copy.** The interesting event is not
  "a block got created" but "the system of record for this item changed from Todoist to
  Holon". Everything else (echo prevention, disposition, provenance) follows from
  making that transfer explicit, typed data.
- **One confirmation primitive.** C5 already defines confirm-promotes: sub-threshold
  origins emit into retractable proposal state; confirmation is an ordinary intent that
  re-emits into canonical state with dual provenance. Assimilation must be an
  *instance* of this, with the connector playing the role of the sub-threshold origin —
  never a second, parallel confirm machine.
- **ADR 0025 conformance:** promotion goes through the operation dispatcher as ordinary
  ops with provenance. The connector's replica tables are read by Holon and written
  only by the sync engine; the assimilated block is written only by ops.
- **Layer-3 ephemerality:** Turso is a rebuildable cache. Any state that must survive a
  reseed (the identity mapping, queued dispositions) has its TRUTH in block data;
  Turso relations over it are derived matviews.

## 1. Substrate inventory (verified 2026-07-12)

What already exists — the design builds on all five:

| Piece | Where | Status |
|---|---|---|
| Connector replica tables | sidecar `entities:` → Turso tables `{prefix}{entity}` (e.g. `todoist_tasks`), synced by `McpSyncEngine` (poll + resource-subscribe), rendered via PRQL (`todoist_hierarchy.prql`) | LANDED |
| Connector **write-back** | sidecar `tools:` (`complete-tasks`, `update-tasks`, `delete-object`…) executed by `McpOperationProvider: OperationProvider` with mirror-undo capture (`crates/holon-mcp-client/src/mcp_provider.rs:140`) | LANDED (immediate tool-call; no queue, no lease) |
| Provenance C2a | `ProvenanceStamp` under `_provenance` property, stamped at the dispatch chokepoint (`crates/holon/src/api/operation_engine.rs`) | LANDED |
| History C2b | `block_history` table via `HistoryStore` (+ `DegradedHistoryStore` for org-standalone) | LANDED |
| Confirm-promotes C5 | `ProposalRecord` blocks under `block:proposals` (`_proposal` property = ONE wrapped op), deterministic proposal ids, `accept_proposal`/`reject_proposal`, gate runs first in `execute_operation`, `_proposed_by` dual provenance | BUILT in stream workspace, pending ratification/weave |

What does NOT exist: any external-id ↔ block-id mapping (verified: zero hits repo-wide),
any queued/outbox shape for external effects, any lease machinery (ADR 0024 P4 external
arm), multi-op proposal payloads.

## 2. The lifecycle as data

Five stages; each stage is a queryable representation, every arrow is either the sync
engine (stage 0) or an op through the dispatcher.

```
[0 REPLICA]  todoist_tasks row            connector-owned, Holon read-mostly
     │  (candidate query: maintained matview, anti-join vs mapping)
[1 CANDIDATE] assimilation_candidates row  VIEW semantics — retracts if item vanishes externally
     │  confirm_assimilation(...)  — ordinary intent
     │      trusted origin → executes;  sub-threshold origin → C5-coerced into a proposal block
[2 PROMOTION] one transactional op batch:  create subtree + mapping property (+ queue disposition)
[3 NATIVE]   block(s), Holon-authoritative; mapping row prevents re-import
[4 SETTLED]  disposition effect executed externally (complete/delete) or Leave
```

### Stage 0 — external replica (exists today)

Sidecar-defined tables, connector is the sole writer (Layer-1 rule). Pre-assimilation,
Holon "edits" to these rows are **write-through tool calls** (the existing
`McpOperationProvider` path): the external system stays authoritative and the edit is
an external effect, not a merge. This is already the live behavior
(`todoist_hierarchy.prql` renders `checkbox`/`editable_text` wired to tools) and it is
the correct pre-assimilation semantics — keep it.

### Stage 1 — candidate (a maintained view, NOT pre-minted proposal blocks)

```prql
from todoist_tasks
filter projectId == {inbox_project_id} && checked == 0
join side:anti assimilation_map (connector=="todoist" && external_id==todoist_tasks.id)
```

In ADR 0024 vocabulary: a read arc over the replica relation plus an **inhibitor arc**
over the mapping relation, emitting into a **display place** — maintained, so a row
retracts automatically when the item is completed/deleted externally or gets
assimilated on another device. Zero stored state, no staleness, no GC.

**Deliberate divergence from a naive reading of C5:** we do NOT mint a `ProposalRecord`
block per inbox item. C5 proposal blocks are *ratcheted* (status flips, rows kept for
acceptance stats) — right for "an untrusted agent tried to do X", wrong for "200 inbox
items exist"; pre-minting would ratchet noise and need GC when items leave the inbox.
Instead the candidate list is view-semantics, and C5 enters exactly where it belongs:
`confirm_assimilation` is an ordinary op, so when a *sub-threshold origin* (an
auto-assimilation rule, a low-trust agent) issues it, the existing gate coerces THAT op
into a C5 proposal block — single-op payload suffices, because the wrapped op is the
compact `confirm_assimilation(connector, external_id, …)` intent, not the expanded
batch. The trust ladder for connectors/rules comes for free; no new gate, no fork.

### Stage 2 — promotion (the confirm intent)

`confirm_assimilation` — an ordinary block-entity op:

```
confirm_assimilation {
  connector:    "todoist"            // sidecar id
  external_id:  "6X7rM8...",         // replica row pk
  template:     "todoist_task",      // block-shape template (sidecar `assimilation:` section, §5)
  target:       ParentRef,           // where the native subtree lands (user-chosen or template default)
  disposition:  Complete | Delete | Leave   // what happens on the external side
}
```

Execution (one dispatch, one transaction — see G2; `db_handle.transaction()` per the
deferred-FK autocommit wart):

1. **Expand template → op batch.** The replica row (+ children rows, e.g. sub-tasks via
   `parentId`) is read and mapped to a block subtree by the template. Every created
   block gets a **deterministic id**:
   `UUIDv5(HOLON_ASSIM_NS, connector ‖ external_id ‖ output-slot ‖ [element-index])` —
   the ADR 0024 P4 internal-effect discipline verbatim. Consequences: re-confirm is
   idempotent; two devices confirming the same item concurrently converge by CRDT merge
   into ONE subtree. Identity continuity is by construction, before any mapping lookup.
2. **Stamp the mapping** (§3): property `assimilated_from: "todoist:6X7rM8..."` on the
   subtree root (+ `assimilated_at`). This is the authority-transfer fact.
3. **Provenance:** the create ops get `_provenance` from the confirmer (existing C2a
   path — user origin, or agent + `_proposed_by` if it went through a C5 proposal).
   `block_history` records the ops. Assimilated-from is NOT crammed into
   `ProvenanceStamp` — the mapping property is the durable, queryable place for it;
   provenance answers *who/when confirmed*, the mapping answers *what it was*.
4. **Queue the disposition** (unless `Leave`): emit an effect block (§4). The external
   call is NOT made inline in the promotion transaction — ADR 0024 P4: external
   effects are once-only and lease-governed; coupling them into the block transaction
   would make a Todoist outage abort assimilation (wrong) or a replay double-complete
   (worse).

### Stage 3 — native + mapped

The subtree is ordinary Holon data. The mapping (matview row derived from the
`assimilated_from` property) now:
- **anti-joins the candidate query** → the item never re-proposes, even though the
  replica row still exists and re-syncs (disposition `Leave`, or `Complete` where
  Todoist keeps completed items);
- marks authority as transferred (§3) → the write-through editing path is disabled for
  this row; the replica row degrades to a shadow.

**Post-assimilation external edits** (disposition `Leave`): the replica row keeps
syncing; if it changes, that is *represented state*, not a merge problem — a
divergence matview (`mapped rows where replica.updated > mapping.assimilated_at`) can
emit maintained "changed externally since assimilation" advice rows. Explicitly a
follow-up, and explicitly NOT auto-merge: authority already transferred; the external
side is informational.

### Stage 4 — disposition settled

The effect executor (§4) runs the queued tool call (`complete-tasks` / `delete-object`
via the existing `McpOperationProvider`), flips the effect block's status. The
Automations-journal query (ADR 0024 P8) shows it: "Assimilated 'Read Loro paper' —
completed in Todoist ⚙".

## 3. Authority as a type, not a convention

The mapping relation (matview `assimilation_map`, truth = block properties):

| column | source |
|---|---|
| `connector`, `external_id` | parsed from `assimilated_from` property |
| `block_id` | the property's owner |
| `authority` | typed enum, from the property that created the row |
| `assimilated_at` | `assimilated_at` property |

```rust
/// Parse-don't-validate: constructed only at the property→matview boundary;
/// no string comparisons downstream.
enum ExternalAuthority {
    /// `linked_to` property: Holon references the item; the external system
    /// remains the system of record. Write-through editing stays ENABLED;
    /// candidate anti-join still excludes it (it was triaged: "leave + link").
    ExternalLinked,
    /// `assimilated_from` property: authority transferred. Write-through editing
    /// for this replica row is a hard Err; replica row is a shadow.
    HolonAssimilated,
}
```

Illegal states unrepresentable:
- *No mapping row* ⇒ external authority, write-through editing, candidate-eligible.
  (The absence case is a state of the anti-join, not of an enum — it cannot be wrong.)
- *`HolonAssimilated`* ⇒ the dispatcher rejects external write-through ops targeting
  that `(connector, external_id)` with a loud `Err` ("assimilated into block X —
  edit it in Holon"), exactly like invariant-3's `sort_key` rejection.
- *Two blocks claiming the same external id* ⇒ prevented for the promotion path by
  deterministic ids (same input → same block id, merge collapses); a manual/imported
  duplicate `assimilated_from` property is a UNIQUE violation in the matview → loud,
  surfaced, never LWW'd (single-ownership discipline, ADR 0025).
- The enum lives on the mapping, not on the block: a block does not know or care that
  it was assimilated; only the boundary to the external system consults authority.

`ExternalLinked` is what makes the enum non-degenerate, and it is Martin's third triage
exit: "keep in Todoist but I want to see/reference it from this Holon project" — a
reference block rendering live from the replica row. Cheap to add once the mapping
exists; v1 may ship `HolonAssimilated` only, but the type is designed for both.

**R1 (ruling needed): does `assimilated_from` round-trip through org?** Recommendation:
YES — plain property, no `_` prefix. It is user-meaningful ("where did this come
from?"), and an org-standalone vault that lost it on round-trip would lose echo
protection and re-import assimilated items on the next sync. ADR 0025 forbids the
implicit middle; this one belongs on the round-trip side. (`_provenance` stays internal
as today.)

## 4. Disposition = queued external effect (outbox)

Truth as blocks (Layer-3 ephemerality): an effect block under `block:effect_queue`,
program-marked (P6, not renderable content), properties:

```
_effect: { connector: "todoist", tool: "complete-tasks", params: {ids: ["6X7rM8..."]},
           status: Queued | Executing | Done | Failed{error},
           effect_id: UUIDv5(HOLON_EFFECT_NS, rule-or-intent-id ‖ firing-key),   // P4
           lease: null }   // reserved: lease epoch/holder when P4 leases land
```

Executor, staged honestly:
- **v1 (single-executor mode, DISCLOSED):** one local drain loop; picks `Queued`,
  flips `Executing`, calls through the existing `McpOperationProvider`, flips
  `Done`/`Failed{error}` (never swallowed — `Failed` rows render in the automation
  journal with the error). Deterministic `effect_id` makes the *enqueue* convergent
  across replicas (two devices confirming → one effect block after merge); the
  *execution* is single-device by disclosed assumption. `Executing` + process death →
  a stale-`Executing` tripwire query (age > TTL) surfaces it; no silent retry.
- **v2 (when multi-device automation is real):** the lease token + TTL + user-override
  machinery of ADR 0024 P4, verbatim — the `lease` slot above is its seat. Nothing in
  v1's data shape needs migration; only the claim step changes from "I am the only
  process" to "I hold the lease".

Sidecar delta (small, declarative): bind triage verbs to tools per entity —

```yaml
entities:
  todoist_tasks:
    assimilation:
      template: todoist_task        # block-shape template, §5
      dispositions: { complete: complete-tasks, delete: delete-object }  # Leave needs no tool
      default_disposition: complete
```

## 5. Block-shape templates

The replica-row → block-subtree mapping is data, not code (this is precisely
VisionGap-C1's "twin definition: resource→block-shape mapping"):
- v1: an `assimilation.template` section in the sidecar (fields → content/properties/
  task-state; child rows → child blocks; Todoist `description` → first child;
  `labels` → tags; `dueDate` → deadline property).
- v2: templates as vault blocks per `docs/Proposals/Templating-2026-07-12.md`, so users
  define/override them at runtime — same trajectory as rules (ADR 0022 precedent).

Multi-block expansion mints per-block deterministic ids via the `output-slot ‖
element-index` discriminators (ADR 0024 P4 already specifies exactly this for
multi-output transitions — no new id scheme).

## 6. Generalization — the assimilation pentad

The reusable primitive set (each independently useful, jointly the Integrator's loop):

1. **Candidate source** — any query, over replica tables OR native blocks, anti-joined
   against the mapping. (= ADR 0024 read arcs + inhibitor arc.)
2. **Proposal surface** — maintained display-place emission for browsing/triage;
   C5 proposal blocks *only* where an untrusted origin acts. (= place-kind distinction,
   already ratified in ADR 0024.)
3. **Confirm-promotes** — one intent expanding to a transactional, deterministic-id op
   batch. (= C5 accept + G2 multi-op payload.)
4. **Identity-mapping relation** — property-truth + matview, typed authority, echo
   anti-join.
5. **Disposition effects** — queued, lease-ready external effects.

Three concrete reuses:

- **Email → task** (gmailcal sidecar already exists as a config precedent): candidate =
  inbox query over the mail replica; template maps subject/body/sender → task block
  with a `linked_to` mapping on the thread; disposition = archive/label. *Identical
  pipeline, different sidecar.* Nothing new to build beyond the sidecar file.
- **Integrator entity-resolution (dedupe person blocks):** candidate source is a
  *native-native* query (C3 registry: `similar(block, k)` / `attr_match(email)` over
  person blocks) — proving primitive 1 does not assume an external replica. The
  "promotion" is a merge op batch (update survivor, re-point edges, delete loser);
  the mapping generalizes to `merged_from: <loser-id>` — identity continuity so old
  references/history resolve. Exercises primitive 3's op-batch generality (ops are not
  only creates) and primitive 4 with both sides internal. Dispositions: none.
- **Inbox-zero triage over ANY connector:** a single triage perspective =
  `union` of every connector's candidate matview, rendered as one queue with verbs
  keep-external / link / assimilate / dismiss. Dismiss = the existing suppression
  anti-join (ADR 0022 machinery) — a fourth exit that costs nothing. This is Martin's
  Todoist workflow generalized to n connectors with zero per-connector UI.

The through-line: **assimilation is the trust-gate pattern with a connector as the
sub-threshold origin.** External data is, in trust terms, an unconfirmed writer; the
confirmation queue IS the trust boundary. One primitive family covers "AI wants to
change my vault" and "Todoist wants into my vault" — which is the substitution test the
vision demands (VisionGap §4 through-line).

## 7. Precise delta to C5 (no fork)

C5's shapes are right; two additive generalizations, both localized to
`crates/holon-api/src/proposal.rs` + `run_resolve_proposal`
(`crates/holon/src/api/operation_engine.rs:584-698` in the C5 workspace):

- **D1 — multi-op payload:** `ProposalRecord { op: WrappedOp }` →
  `ProposalRecord { ops: Vec<WrappedOp> }` (`_proposal` JSON gains an array;
  single-op stays the `len==1` case). `run_resolve_proposal`'s one recursive
  `execute_operation` call becomes a loop inside one transaction; abort-on-first-Err,
  nothing partial. Needed by: assimilation-via-untrusted-origin *if* we ever wrap the
  expanded batch; merge proposals (Integrator) definitely. Note: for assimilation the
  cheap path wraps the *compact* `confirm_assimilation` op (one op — no D1 needed);
  D1 is driven by the merge/general case, so it can land after C5 weaves.
- **D2 — nothing else.** Disposition side-effects do NOT need a `ProposalRecord.on_accept`
  hook: the wrapped op is `confirm_assimilation`, whose own execution enqueues the
  effect block. Keeping effects inside the promoted op (not the proposal envelope)
  preserves C5's invariant that accept = plain re-dispatch with confirmer origin.
  Likewise the mapping row is written by the op batch itself (it's just a property) —
  the `mapped_identity` field the C5 recon speculated about is unnecessary.

## 8. Gap ranking (missing fundamentals, by unlock/effort)

| # | Gap | Effort | Unlocks | Verdict |
|---|---|---|---|---|
| G1 | **Identity-mapping relation** as first-class: `assimilated_from`/`linked_to` properties + `assimilation_map` matview + `ExternalAuthority` enum + dispatcher authority check | **S** | Echo prevention, authority transfer, LINKED state, Integrator `merged_from`, every reuse in §6 | **Build first.** Pure existing machinery (property + IVM matview + one dispatch check) |
| G2 | **Multi-block transactional promotion**: compound op batch, one transaction, deterministic ids per element | **M** | Real assimilation (task+description+subtasks), Integrator merges, C5-D1, future grouped undo | Second. The id scheme is specified (ADR 0024 P4); the work is the batch dispatch path + FK-wart-safe transaction |
| G3 | **Outbound effect queue + v1 single-executor drain** (lease-*ready* data shape, lease machinery deferred) | **M** | Dispositions, and the seat for ALL future external effects (ADR 0024 P4 arm — send-email, calendar writes) | Third. Without it dispositions are inline tool calls: fine single-device, undisclosed dupe risk the moment two devices confirm |
| G4 | **Sidecar `assimilation:` section** (template + disposition verb binding) | **S** | Per-connector onboarding drops to editing one YAML | With G2. Trivially declarative; v2 = vault-block templates |
| G5 | **Full lease governance** (ADR 0024 P4: lease token, TTL, takeover, reconciliation) | **L** | Multi-device automation safety | **Defer.** v1 disclosure + G3's reserved `lease` slot means zero migration later |

Not gaps (verified existing): connector write-back API (sidecar `tools:` +
`McpOperationProvider` — G4 only *binds* it), provenance/history (C2a/C2b landed),
confirm-promotes core (C5 built, pending weave).

## 9. Rulings needed from Martin

- **R1** — `assimilated_from` round-trips org (recommended) vs `_`-internal. §3.
- **R2** — candidates as maintained view + NO pre-minted proposal blocks for
  user-confirms; C5 blocks only via gate coercion of untrusted origins (recommended —
  keeps C5 unforked, §1 stage 1). Alternative: every candidate is a C5 proposal block
  (uniform but ratchets noise + needs GC).
- **R3** — default disposition for the Todoist inbox flow: `complete` (recommended —
  keeps Todoist history, cannot destroy data) vs `delete` vs `leave`.
- **R4** — sequencing of C5-D1 (multi-op payload): into the C5 stream pre-weave, or as
  a follow-up once woven (recommended: follow-up; assimilation's untrusted path wraps
  the compact op and doesn't block on it).

## 10. Spike status

**Deliberately not built.** The C5 machinery this composes with is unwoven (pending
ratification in its stream workspace); the task's stop-rule ("if C5 isn't woven, stop
at doc + interface sketch rather than building a parallel mechanism") applies. §2–§4
carry the interface sketches (op shape, mapping schema + enum, effect-block shape,
sidecar delta). First implementation increment after C5 weaves + rulings: G1 with a
fixture-based directed test (fake `todoist_tasks` rows, no network → candidate matview
→ `confirm_assimilation` → subtree + mapping row + queued effect block; assert
anti-join removes the candidate and re-confirm is a no-op by id determinism).
