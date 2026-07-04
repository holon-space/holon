# ADR 0029: Entity identity has one minting authority per family, lint-enforced

**Status:** **Principle accepted (2026-07-26); D1/D2 RATIFIED AS AMENDED
2026-07-26 (Martin, live review).** Martin ruled the principle: *"entity identity
must have a single authority, as an architectural concern. We had this issue
multiple times and had actually decided to have one authority, but the
implementation seems to have diverged."*

In a live in-session review on 2026-07-26 Martin then reviewed D1 and D2 and
**ratified both as amended**. The amendments are folded into the sections below
and are the *only* deltas from the originally-proposed text; the census, the
owner assignments (now cut by derivation class -- D1), the D2 prohibitions, the
ratification of `PageId::for_page_under`, and the recorded page precedence chain
now stand as **decided**, not proposed. D1/D2 may be cited as settled.

**Deciders:** Martin (principle; D1/D2 as amended, 2026-07-26 live review).
**Promotes / supersedes (scope):** `docs/Plans/PageIdentityDeterminism.md` §5
(ACCEPTED 2026-07-18) — that ruling stays correct and in force for *pages*; this
ADR is the general rule it was a special case of, and it amends the ruling's §5.1
claim that `PageId::for_path` is "the ONE sanctioned way" (a second sanctioned
constructor, `for_page_under`, landed afterwards without amending the doc).
**Relates to:** ADR 0014 (`doc:` scheme retirement — `block:` is the only entity
scheme; page-ness is the `Page` tag), ADR 0015 §1a (entity identity vs element
identity), ADR 0016 (occurrence-keyed focus identity — still **Proposed**),
`docs/Architecture/Model.md` invariants 2/10/11,
`docs/Architecture/Replication.md` §"ID policy" (line 59),
`archlint/smells/order_minting.toml` (the precedent this ADR copies).

---

## Problem

### Identity is minted everywhere and owned nowhere

An exhaustive census of every site in the tree that *decides what a thing's id
is* (mint, derive, adopt, default, override, re-mint), taken at
`main = 6252d5003e47`, found **19 production block-id minting sites across 8
crates, producing 5 distinct id shapes**, with no owning type, no architectural
invariant, and no lint. Among them:

- `EntityUri::block_random()` (`crates/holon-api/src/entity_uri.rs:77`) is the
  nominal random-mint primitive — but four other sites mint the same shape by
  hand-formatting the scheme instead of calling it:
  `crates/holon/src/core/sql_operation_provider.rs:2436`
  (`format!("{}:{}", self.entity_name, uuid)`),
  `crates/holon-core/src/traits.rs:1232-1233` (`split_block`),
  `crates/holon-loro/src/loro_share_backend.rs:1355` and `:1695` (mounts).
  Hand-formatting bypasses `EntityUri::new`'s double-scheme guard
  (`entity_uri.rs:40-48`), which is in any case a `debug_assert!` and therefore
  absent from release builds.
- `impl Default for Block` mints an id implicitly
  (`crates/holon-api/src/block.rs:375`). Every `..Block::default()` produces a
  fresh uuid invisibly; `crates/holon-org-format/src/parser.rs:381` and `:627`
  immediately overwrite it — a mint-then-discard that would silently become a
  live mint if a future caller forgot the explicit `id`.
- A frontend mints entity identity directly:
  `frontends/gpui/src/views/editor_view.rs:1507` calls `EntityUri::block_random()`
  in the GPUI view for clipboard-image paste. `Replication.md:59` says the UI's
  ID policy is "—", i.e. it mints nothing.
- Identity is derived from *position* for four child families —
  `block:{parent}::src::{index}` (`parser.rs:361-364`),
  `::img::{index}` (`parser.rs:475`),
  `block:{file_id}::b::{seq}` (`crates/holon-markdown/src/logseq.rs:235-238` and
  `crates/holon-markdown/src/obsidian.rs:194-200`) — so reordering re-assigns
  identity.
- `crates/holon-loro/src/loro_backend.rs:3245-3260` (`set_external_id`)
  overwrites an existing block's `STABLE_ID` in place from a foreign system's
  id, while the neighbouring doc-comment at `:3236` states external ids are "NOT
  used for block identity". No production caller today; it is public API.

Page identity, by contrast, *was* ruled: `PageId`
(`crates/holon-api/src/link_parser.rs:131`) is a genuine newtype whose scheme is
not caller-chosen and whose constructor is fail-loud. Even there, **six other
authorities can decide the same page's id**: the companion `#+ID` adoption
(`crates/holon-filesystem/src/file_sync_controller.rs:1183-1192`), the file
`#+ID:` override (`crates/holon-org-format/src/parser.rs:103-115`,
`:140-143`), the content lookup `resolve_page_name`'s `ORDER BY … LIMIT 1`
(`sql_operation_provider.rs:1054`), the leaf-suffix `LIKE '%/name'` re-resolution
(`sql_operation_provider.rs:1432-1450`), the id-substituting
`LiveDocumentManager::create` (`crates/holon-app/src/turso_seams.rs:552-566`),
and `PageId::from_segments` reached directly from `classify_link`
(`link_parser.rs:268`), which bypasses `for_path`'s empty-segment guard.

Two of those are the "replacement that never removed what it replaced" hazard.
`resolve_dir_page_chain` (`file_sync_controller.rs:1168`) documents itself as the
companion-aware replacement for `get_or_create_by_name_chain`
(`crates/holon-filesystem/src/sync_ports.rs:271`), but the replaced method is
still on the `DocumentManager` trait and still reachable; the two create through
different paths (`create_forcing_id` at `sync_ports.rs:162` honours the supplied
id, `create` at `turso_seams.rs:552` discards it at `debug!` level). **The caller
picks which authority applies.**

### Why identity drifted and order did not

Ordering is the exact structural analogue of identity, and it did *not* drift.
The difference is enforcement, not doctrine:

| | ruled monopoly | stated in Model.md invariants | archlint rule |
|---|---|---|---|
| **fractional index / order** | yes (consolidator, "Monopolist of order") | yes (invariants 2 and 10) | yes — `archlint/smells/order_minting.toml` |
| **entity identity** | yes, for pages only (2026-07-18) | **no** | **no** |

`Model.md`'s invariants 1–12 contain no identity invariant; the closest is
invariant 11's mount-identity clause ("the mount id ≡ the shared page's id") and
invariant 12 listing `id` among the derived/control fields. The single most
important identity rule in the system is absent from the page every agent is told
to load first. The only identity-adjacent lints
(`entity_uri_from_raw`, `entity_uri_parse_default`, `focus-no-from-raw` —
`docs/Architecture/Archlint.md:230-232`) police *re-parsing*, not *minting*.

That asymmetry is the mechanism of the drift, and it is why point-fixes have not
held: nothing prevents the twentieth minting site from appearing tomorrow.

### The motivating incident: 34% of the live vault is duplicate rows

Measured 2026-07-26 against a read-only copy of Martin's live
`~/.config/holon/holon.db`, queried with `tursodb`:

- **17,052 blocks**, of which **5,787 are duplicate excess rows** — roughly
  **34% of the store**.
- **19 duplicated `(content, parent_id)` groups.** The largest holds **721
  copies of one block**: 721 distinct ids, 721 distinct `created_at` values
  spanning about 5.5 hours, all at the same depth under the same parent, each
  carrying its own fractional index.

That signature is a **re-mint-per-reparse pump**, not a merge artifact: every
re-ingest minted a NEW id for the same headline and appended it as a new sibling.
The mechanism is known and documented in the tree — an `:ID:`-less org headline
is minted a fresh `Uuid::new_v4()` on **every** parse
(`crates/holon-org-format/src/parser.rs:741`), acknowledged verbatim at
`crates/holon-filesystem/src/file_sync_controller.rs:1660-1671`. It is §5.5
carve-out #1 of the 2026-07-18 ruling: deliberately left open.

**The re-mint is necessary but not sufficient.** Minting a fresh id on one parse
yields one duplicate, not 721. Producing 721 requires the file to be re-ingested
721 times, which is the second half of the pump: a file whose ingest fails never
enters `last_projection`, so the 2-second `discovery_tick`
(`crates/holon-orgmode/src/di.rs:1036-1045`) re-discovers and re-ingests it
forever, and the `?` at `crates/holon-filesystem/src/file_sync_controller.rs:3368-3378`
propagates out of the discovery walk so every file later in walk order is never
discovered at all. Identity supplies fresh ids; the ingest loop supplies the
repetitions; order minting files each one as a new sibling. Each mechanism is
locally defensible, which is why it ran unobserved for hours. Any remediation
that closes only the identity half will keep duplicating, just once per genuine
edit instead of once per tick.

The ruled repair does not reach this. `SqlOperationProvider::dedup_pages`
(`sql_operation_provider.rs:1273`) collapses duplicate **`Page`** groups; the
vault's page-level duplication is a **single pair** ("Prototype"). Running it
today would fix **0 of the 5,787 rows**. It also has **no production caller** —
`dedup_pages` is invoked only from
`crates/holon/tests/create_page_from_link.rs:470`, `:513`. The repair Martin was
promised for his vault shipped as a function nothing calls, aimed at a family
that is not the one duplicating.

The mitigation that *does* run is a heuristic, not an authority:
`compute_idless_remaps` / `tiered_match`
(`file_sync_controller.rs:4427`, `:4635`) reconcile id-less headlines by a
three-tier content/position/subtree-fingerprint match whose tier 3 always claims
a candidate. Identity for those blocks is decided by **content similarity**. It
is disclosed (a `MatchBasis` is recorded, a WARN is emitted at
`file_sync_controller.rs:1800-1810`), which is the right behaviour under the
fail-loud rule — but it is still a guess standing where an authority should be.

---

## Decision

**Entity identity is an owned architectural resource. For each identity family
there is exactly one type that may mint, and exactly one boundary at which
minting happens. Everywhere else, ids are only carried.**

The model to generalize already exists in the tree:
`crates/holon-api/src/effect_id.rs`. Four id families (rule effect, template
instance, trust proposal, connector intent key), each with a fixed checked-in
UUIDv5 namespace, each derived from typed newtype inputs (`RuleId`, `FiringKey`,
`OutputSlot`), each with exactly one minting function and **no fallback arm** —
`deterministic_block_id` (`:85`), `deterministic_instance_id` (`:106`),
`deterministic_proposal_id` (`:126`), `deterministic_intent_key` (`:149`).
`FiringKey::from_row` (`:50-62`) even excludes `_`-prefixed CDC columns because
their `Value` type differs between the Created and Updated paths and would
otherwise "mint path-dependent ids and break cross-replica convergence". That is
the standard every other family is now held to.

### D1 — Owners, by derivation class

**[Ratified as amended 2026-07-26.]** The reframing Martin ruled: the owning
property belongs to the *derivation*, not to the *kind of entity*. Families are
therefore cut by **how the id is derived**, and each derivation class mints
through exactly one sanctioned constructor. (Page-vs-block is *not* the cut:
page-ness is the `Page` tag, ADR 0014, not an identity family — see the
construction-witness note below.)

| Derivation class | Owning constructor | Current users | Notes |
|---|---|---|---|
| **(a) convergent-by-path** | `PageId::for_path` (`crates/holon-api/src/link_parser.rs:158`), `PageId::for_page_under` (`:181`) | pages, journals | Same inputs → same id on every peer. Both funnel through the single canonicalization `segments` (`:138`) → `from_segments` (`:143`). `for_page_under` is hereby **ratified** as a sanctioned sibling of `for_path` (it treats the leaf as one segment because it comes from block *content*, a title, not a path). **The property belongs to the derivation, not to page-ness** — any name-addressable entity minted this way converges; today's users happen to be pages and journals. `from_segments` stays private-in-effect: `classify_link`'s direct call (`:268`) is a violation to burn down (see OQ2). |
| **(b) unique-random** | `EntityUri::block_random()` (`crates/holon-api/src/entity_uri.rs:77`) | non-page blocks | Uniqueness by construction, no convergence. Hand-formatted `format!("block:{…}")` / `format!("{}:{}", entity_name, uuid)` are forbidden — go through the primitive. Which *component* is the minting boundary is settled by D1c (mode-selected executor, OQ1 resolved). |
| **(c) deterministic-by-typed-inputs** | `crates/holon-api/src/effect_id.rs` (the four functions) | rule effect, template instance, trust proposal, connector intent key | Already exemplary; unchanged. Cited as the pattern for the whole ADR. |
| **(d) positional** | — (no sanctioned owner) | `parser.rs:361-364` (`::src::`), `:475` (`::img::`), `crates/holon-markdown/src/logseq.rs:235-238` / `obsidian.rs:194-200` (`::b::`) | **Reclassified as a DEFECT CLASS to burn down, not a family.** Deriving identity from ordinal position means reordering re-assigns identity. No sanctioned constructor exists or will; every site is debt (burn-down sequencing in OQ4). |
| **(e) doc / file id** | `generate_file_id` (`crates/holon-org-format/src/parser.rs:44-51`) | transient parse-time only | Unchanged. `generate_file_id`, `generate_file_id_from_relative_path` (`:54-56`). `file:` is **transient, parse-time only** (ADR 0014): a `file:` id that reaches the DB is a defect. |

**`PageId` is a construction-witness, not a second id type.** `PageId` is a
newtype *witness* over the single `block:` id space that records **how** an id
was constructed (the canonical path derivation); **there is no second id type at
rest**. Page-ness is exclusively the `Page` tag — consistent with D2 prohibition
5 and ADR 0014. "Convergent-by-path" is a property of the derivation the witness
attests to, not of the entity being a page.

Out of scope, deliberately: **element identity** (`(EntityUri, Occurrence)`,
ADR 0015 §1a / ADR 0016) is a render-slot identity, not entity identity, and
`RowIdentity` (`crates/holon-api/src/widget_spec.rs:195-198`) is query-result row
keying. Both are correct as they stand and are not governed by this ADR. So is
the cross-system entity *resolution* concern (`IdentityProvider`, merge/propose/
accept/reject) tracked in the vault as G2 work — that is entity **matching**, not
id **minting**, and conflating the two has already caused confusion.

### D1b — Collision policy

**[Ratified 2026-07-26.]** When a convergent-by-path derivation yields an id
already held by a **different** entity:

**Interim (ships first): FAIL LOUD.** The create is refused with a visible
error; no silent substitution. The collision predicate is precise: the id's
current holder's canonical path/title ≠ the requested (normalized) path — a
path/title comparison under `PageId::for_path`'s normalization, **not** a
`(content, parent)` comparison (content drifts over time and would misclassify).
Autonomous callers — in particular the journal auto-create rule, which fires on
a periodic tick — must not error-storm: they treat a collision-`Err` as
**already-satisfied**, skipping with a single disclosed log rather than
re-raising an error every cycle.

**End-state (RULED 2026-07-26):** the **first** creation of a name-addressable
entity stays **convergent-by-path**. A **recreate at a freed/collided path mints
UNIQUE-RANDOM** and binds the name to that id through the existing name→id
resolution machinery (`resolve_page_name` / the junction `resolved_id`) — so the
*binding* converges by CRDT merge instead of the *id* converging by
construction. Cross-peer concurrent recreate-at-the-same-freed-path then yields
two random ids, the rare shape the existing `(name, parent)` repair already
covers, confined to that corner instead of being the general regime.

**The prior open question — a "convergent disambiguation input" (a suffix such
that every peer disambiguates a collision identically) — is RESOLVED AS
DISSOLVED.** It is structurally unanswerable for independent creations on
different peers: the creation events differ, so any provenance / generation-count
/ rename-tombstone key necessarily diverges (it asks for consensus without
coordination); and for the *same* event re-entering it is a **recognition**
failure that must never reach disambiguation at all. Full analysis:
`~/.claude/plans/holon-identity-fable-review-2026-07-26.md` (F1, §5).

### D1c — Minting topology (resolves OQ1)

**[Ratified 2026-07-26.]** There is **one** minter trait with **three
mode-selected implementations**: a Turso-backed impl, a Loro-backed impl, and a
standalone **pure** impl (unique-random only; for storeless contexts such as the
`holon-markdown` converters). The active implementation is reached through a
consolidator DI accessor cloning the `order_key_minter` pattern
(`crates/holon-core/src/traits.rs:886`). Org ingest is **not** a fourth minter:
a separate org **INGEST-RESOLUTION** trait resolves `Recognized | AdoptForeign |
NeedsMint`, and its `NeedsMint` arm delegates to the *active* minter —
consistent with `Replication.md`'s ID-policy row (org = AcceptForeign +
mint-on-new).

**The principle, stated explicitly: the mode selects the mint EXECUTOR, never
the id VALUE.** Ids must agree across modes and peers; order never had to.
Derivation is mode-independent by construction; the consolidator only selects
*who executes* the mint (whose store answers the recognition/collision query and
records it atomically). A consolidator/epoch handover (`Model.md` invariant 10)
is therefore identity-preserving — only order re-keys.

### D2 — One minting boundary

Minting happens at the boundary where a thing first enters the system, once, and
is recorded. Concretely, and stated as prohibitions because that is what is
checkable:

**Forbidden:**

1. **Re-minting an existing entity's id.** Identity is assigned once, at
   creation. A rename is an ordinary edit to the existing entity
   (`PageIdentityDeterminism.md` §5.3); a re-parse of an unchanged file, a
   replay, an undo/redo, or a re-ingest must reproduce the *same* id, never a
   fresh one. The 721-copy group is exactly this rule being violated.
2. **Hand-formatting an id string.** `format!("block:{…}")`,
   `format!("{}:{}", entity_name, uuid)`, and string-level prefixing such as
   `ensure_block_prefix` (`frontends/mcp/src/tools.rs:179-185`) bypass the
   constructor and its guards. Go through the owning type.
3. **Minting outside a sanctioned boundary component.** In particular, a
   frontend never mints entity identity
   (`frontends/gpui/src/views/editor_view.rs:1507` is a violation to burn down),
   and `Default` impls never mint (`crates/holon-api/src/block.rs:375`).
4. **Adopting an id from a non-boundary source, or overwriting an existing id in
   place.** `set_external_id` (`crates/holon-loro/src/loro_backend.rs:3245-3260`)
   rewriting `STABLE_ID` from a foreign system's id is the canonical example.
   Foreign ids belong in a mapping (`Replication.md:59`'s `OwnForeign(map)`
   policy), never written over an entity's own identity.
5. **Encoding page-ness, or any other classification, in the id.** ADR 0014:
   page-ness is the `Page` tag, exclusively.
6. **Silent substitution or defaulting.** `LiveDocumentManager::create`
   (`turso_seams.rs:552-566`) discarding the caller's `PageId`-minted id at
   `debug!` level violates the project's fail-loud rule; so does
   `EntityUri::from_raw` (`entity_uri.rs:194-218`) silently re-scheming an
   unparseable string as `block:<garbage>`.

**Required where an override is legitimate:** a *stated precedence chain*, in
this ADR or an amendment, not only in code comments. Today's page precedence,
reconstructed from the code, is:

> companion-folder `#+ID` ≻ file `#+ID:` ≻ `PageId::for_path`

— with the disclosed non-resolution that an **already-existing** `(parent,
title)` page beats all three, and the disagreement is only WARN-logged
(`file_sync_controller.rs:1196-1220`). That precedence is hereby recorded as the
intended rule; the unresolved contention case is OQ3.

### D3 — The invariant is stated where agents read it

`docs/Architecture/Model.md` gains an identity invariant (13) stating the
minting monopoly, worded parallel to invariant 2's "Monopolist of order". Until
that lands, this ADR is the statement of record. (This ADR does not itself edit
Model.md.)

---

## Enforcement

**A decision without a lint is not a decision — that is the whole lesson of the
order/identity asymmetry.** This ADR is not satisfied until the lint exists.

### The lint

A new smell `archlint/smells/identity_minting.toml`, modelled directly on
`archlint/smells/order_minting.toml`, with the same four structural elements:

1. **A pattern** matching id minting: `Uuid::new_v4` in a context that produces
   an entity id, `EntityUri::block_random`, and hand-formatted
   `format!("block:{…}")` / `format!("{}:{}", …uuid…)` shapes.
2. **A named owner in the header comment** — the `## Decision` table above, with
   the same "route the intent through the owner instead" instruction that
   `order_minting.toml` gives for `BlockOrdering`.
3. **An explicit `exclude` list enumerating today's sites, each with a written
   reason.** The defining modules (`entity_uri.rs`, `link_parser.rs`,
   `effect_id.rs`) are permanent exclusions; every other entry is a debt entry
   naming why it is still there. Non-entity uuid mints (session handles, temp
   paths, watch tokens — `crates/holon-loro/src/container_registry.rs:268`,
   `frontends/mcp/src/server.rs:407`, `:440`, and similar) are excluded as
   out-of-family, not as debt.
4. **An `// ALLOW(identity_minting): <reason>` escape** for in-line
   justification, matching `// ALLOW(order_minting): <reason>`.

**The exclusion list only ever shrinks.** Adding a file to it is a reviewable
change that must carry a reason; removing one is the burn-down.

### Recognition (resolve-before-mint) is a first-class operation

**[Added 2026-07-26.]** The lint and the owner table close the *minting* half of
the problem; they do not close the half that produced the measured damage. Every
boundary that can re-observe an existing entity (org re-ingest, MCP org-patch,
share-mount accept, redo graft, sync echo) must **resolve before it mints** —
emitting `Recognized(existing_id) | AdoptForeign(id) | NeedsMint`, with `mint`
reachable only from the `NeedsMint` arm. The damage class this catches is
**different-id-same-entity**, which a collision check *cannot* see: a fresh
random id never collides with anything, so nothing at the mint site stops a
sanctioned caller from minting for a thing that already exists. Recognition is
the cure; the lint and traits are prophylaxis against the next mint site.

### Witness types complement the lint at the type level

**[Added 2026-07-26.]** The lint sees textual mint *expressions*, not id data
*flows* — so `let id = address_of(path); create(id, …)` launders a derived id
through sanctioned functions past the lint. Witness types close that back door:
`mint` returns an unforgeable `MintedId`; parse boundaries yield a `CarriedId`
via a small set of blessed constructors; `create` accepts only
`Minted(MintedId) | Carried(CarriedId)` (a bare `String` / `EntityUri` id no
longer typechecks into a create); and `address_of` returns a non-creatable
`ResolvedAddress` usable only for read-side lookup and link resolution. This
closes the caller-supplied-id-string back door that a regex structurally cannot
see.

### Precondition: the arch gate cannot currently fail

`justfile:475-476`:

```
analyze-arch:
    ./archlint/archlint --all 2>&1 | tee /tmp/holon-analyze-arch.log
```

The recipe pipes into `tee` without `set -o pipefail`, so the recipe's exit
status is `tee`'s — **always 0**. `analyze-arch` reports green on a red archlint
run. This is the same false-green class as the four recipes fixed in
`2b021fd3f8`. **Fixing this recipe is a precondition for the `identity_minting`
lint to mean anything**; until it is fixed, adding the lint changes nothing that
CI can observe. (This ADR does not itself edit the justfile.)

---

## Consequences

- **The burn-down is incremental and independently landable.** Each of the ~19
  block-id sites is a separate exclusion entry; each removal is a small PR that
  routes one site through the owner and deletes one line from the toml. No big-
  bang refactor, and progress is measurable as a monotonically shrinking list.
- **Stopping the pump precedes any dedup migration.** Deduping the vault while
  `parser.rs:741` still re-mints on every parse just refills it — the 5.5-hour
  spread of the 721-copy group is the measurement of how fast. Order of work:
  (1) close the id-less-headline re-mint carve-out **and** the re-ingest loop
  that drives it (both halves -- see the motivating incident), (2) verify on the
  live vault that duplicate counts have stopped growing, (3) only then run a
  dedup migration, and that migration must cover **blocks**, not only `Page`s,
  since `dedup_pages` would fix 0 of the 5,787 rows. The block-level migration
  needs a dry-run mode, which `dedup_pages` does not have: the tail of the
  duplicate distribution is ambiguous (`Frontends` x4, `Holon` x2 may be
  legitimate) where the 721- and 365-copy groups plainly are not.
- **`dedup_pages` needs a decision.** It has no production caller. Either it
  gains one (boot health sweep / MCP maintenance op) or it is deleted as
  unreachable; leaving a ruled repair as dead code misrepresents the system's
  state to the next agent.
- **Two page-creation routines must become one.** `resolve_dir_page_chain` and
  `get_or_create_by_name_chain` cannot both stay reachable — a replacement that
  did not remove what it replaced is exactly the hazard the project's own
  refactoring rule forbids.
- **The PBT `IdResolver` is the measurement of the drift, and should shrink with
  it.** `crates/holon-integration-tests/src/pbt/op_write_cap.rs:43-47` exists
  solely because the SUT mints ids the oracle cannot predict; its own doc says so
  (`:53-56`). Relatedly, a test-only id shape (`block::split-N`) has forced a
  carve-out into the *production* URI parser (`entity_uri.rs:206-213`) — prod
  identity semantics coupled to test scaffolding, which the burn-down should
  retire.
- **Some cost is accepted.** Routing every mint through an owner is more
  ceremony than `Uuid::new_v4()` at the call site, and the lint will occasionally
  fire on a legitimate new case. The `ALLOW` escape plus a reasoned exclusion
  entry is the intended cost — the same trade `order_minting` already makes, and
  order has not drifted since.

---

## Open questions

These are recorded as unresolved rather than invented, because the census found
no ruling for them and this ADR will not manufacture one.

- **OQ1 — RESOLVED 2026-07-26 (see D1c).** The block-id minting boundary is
  mode-dependent: **one** minter trait with **three** mode-selected executors
  (Turso-backed, Loro-backed, and a standalone pure impl for storeless
  contexts), reached through a consolidator DI accessor cloning `order_key_minter`;
  org ingest *resolves* (`Recognized | AdoptForeign | NeedsMint`) rather than
  mints, delegating its `NeedsMint` arm to the active minter. The mode selects
  the mint *executor*, never the id *value* — ids must agree across modes and
  peers; a consolidator handover is identity-preserving.
- **OQ2 — the `from_segments` bypass in `classify_link`** (`link_parser.rs:268`)
  produces an optimistic link-target id for input that `for_path` would reject
  (`[[a//b]]`). The write is refused downstream, but the link *mark* still
  carries the ghost id. Fail-loud at parse, or keep the optimistic id and accept
  a mark that can never resolve?
- **OQ3 — the companion-`#+ID`-vs-existing-page contention has no runtime
  resolution** (`file_sync_controller.rs:1196-1220`): first writer wins, the
  disagreement is WARN-logged, repair is deferred to a migration that does not
  run. What is the resolution rule?
- **OQ4 — positional identity.** Four families derive identity from ordinal
  position (`parser.rs:361-364`, `:475`, `logseq.rs:235-238`,
  `obsidian.rs:194-200`), so reordering re-assigns identity. Two different infix
  conventions (`::src::`/`::img::` vs `::b::`) exist for the same idea. Is
  positional derivation acceptable for these child families, or is it a defect
  class to close?
- **OQ5 — the two `block:`-scheme identity spaces inside Loro.** A node has both
  a `STABLE_ID` (`block:<uuid>`) and a TreeID address
  (`block:{peer}:{counter}`, `loro_backend.rs:299-303`), and
  `resolve_to_tree_id` (`:3209-3220`) picks between them by *string shape*. No
  live cross-crate caller today, so it is latent — but the resolver precedence
  runs on every write.
- **OQ6 — three id-string normalizers with different strictness read the same
  `id` column:** `entity_uri_from_id_str` (permissive,
  `widget_spec.rs:56-60`), `row_id`/`uri_from_row` (strict `Result`,
  `crates/holon-api/src/lib.rs:661-682`), and `ensure_block_prefix`
  (string-level, `frontends/mcp/src/tools.rs:179-185`). Which is canonical?
- **OQ7 — `EntityUri::from_raw` is still infallible** (`entity_uri.rs:194-218`),
  silently inventing `block:<garbage>`; the fail-loud conversion to `Result` was
  chosen but never landed (184 call sites). Related: `EntityUri::new`'s
  double-scheme guard and `EntityName::new`'s scheme-validity check are both
  `debug_assert!` (`entity_uri.rs:40-48`, `crates/holon-api/src/types.rs:31-55`)
  and therefore absent in release builds — and `EntityName` feeds the block-id
  mint at `sql_operation_provider.rs:2436`.
- **OQ8 — `holon-sharing`'s identity types are unencapsulated.** `BlockId`,
  `ContainerId`, `CrossingId`, `PolicyEditId` are `pub String`
  (`crates/holon-sharing/src/types.rs:13-14` and following) while the module doc
  three lines above claims parse-don't-validate. They are a parallel string space
  with no checked conversion to `EntityUri`.
- **OQ9 — the block→page identity fork was never ruled.**
  `docs/Plans/BlockToPageTransform-Options-2026-07-17.md:282` poses it (preserve
  the block's id, or mint a new page id?); the implementation chose "mint a new
  page id" via `PageId::for_page_under`
  (`sql_operation_provider.rs:3020`). D1 ratifies the *constructor*; whether that
  was the right semantic choice is still Martin's call.

---

## Alternatives rejected

**A — Keep point-patching each defect as it is found.** This is what has been
happening, and the patches are individually correct. The redo re-mint fix at
`crates/holon/src/api/operation_engine.rs:1237-1251` is the clearest example: a
`create` whose caller omitted `id` has one minted by the provider, the stored
forward op lacks it, so redo would re-mint and dangle every reference — the fix
grafts the minted id off the *inverse* op onto the redo op. It works. It is also
gated on `op_name == "create"` (`:1245`) and fires only on the single-op branch,
so it does not compose: `split_block` (`traits.rs:1232`) also mints and returns
its id in the op *response* rather than in params, and is not covered. Every such
patch reconstructs, after the fact, an id that a single minting boundary would
never have lost. Rejected: it treats the symptom, does not shrink the surface,
and — as the 5,787 duplicate rows show — loses the race against the pump.

**B — Do nothing.** Rejected on the measurement. 34% of the live vault is
duplicate rows, the largest group has 721 copies of one block, and the ruled
repair addresses a family that accounts for one duplicated pair. The trend is
also wrong: the 2026-07-22 fixes added a *fourth* reconciliation mechanism
(content+position remap) rather than closing the carve-out that makes
reconciliation necessary. Each mechanism is defensible alone; together they are
four ways a block can acquire an id, and the next agent reading the tree will
copy whichever one it finds first.

**C — Add a hard `UNIQUE(content, parent)` database constraint.** Already
rejected on the merits by `PageIdentityDeterminism.md` §5.5 and not reopened
here: under CRDT union-by-id it could reject a legitimate merge. The regression
gate is the green invariant PBT, and now the `identity_minting` lint.
