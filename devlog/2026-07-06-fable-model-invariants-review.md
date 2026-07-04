# Deep review: Model.md invariants 1-12 vs reality (Fable, 2026-07-06)

Read-only architecture review. Primary target `docs/Architecture/Model.md`; cross-checked
against Principles/Sync/Replication/Storage/Schema/RenderPipeline/UI/Engine and ADRs
0001/0003/0005/0010/0012/0015, then verified the load-bearing invariants against code
(three parallel code-verification passes + spot checks). All file:line refs are current
working-copy.

**Headline: Model.md is the most accurate document in the tree.** Nearly every checkable
claim in it verified TRUE against code. The rot is concentrated in the *detail* docs
(Replication.md, Sync.md) and in ADR status lines — consistent with Model.md's own
meta-rule ("this page states the intent and the detail doc may be stale"). The real
architectural risks are two invariants that are runtime-convention rather than
type-enforced (inv 3, inv 8), one silent-fallback totality hole (inv 2/4), and one
vacuous invariant (inv 9).

---

## 1. The twelve invariants, restated crisply

| # | Statement (compressed) | Verdict |
|---|---|---|
| 1 | Each replica diffs against its own persisted base, never against the Turso cache | **Enforced in code** (BaseStore seam; verified) |
| 2 | One consolidator per sibling-set owns order and mints every fi; sinks store it verbatim | **Mostly enforced**; one live frontend violation + silent-A0 hole |
| 3 | Inbound intent carries `after_sibling`, never an order key; `set_field("sort_key")` is a hard error | **Runtime-only, mode-dependent**; leaks exist |
| 4 | Exactly one writer per store; projection is total | **Lint-enforced writer**; totality NOT total (silent A0) |
| 5 | Sinks never re-merge; consolidated result is fait accompli | Convention only; no fence, but no violation found |
| 6 | Causality is inherited (scalar base now, Loro/git DAG later), never hand-rolled | Untestable as stated; no hand-rolled VV found (true today) |
| 7 | Loro-the-store and text-merge are decoupled capabilities | **True by construction** (`TransientTextMergeProvider`, `crates/holon-loro/src/text_merge_provider.rs`) |
| 8 | Structural ops are commit points: pending editor state flushes first, in ONE ordered dispatch | **Enforced in GPUI + dioxus-web; NOT in TUI**; convention, not types |
| 9 | Tombstones outlive every base (no GC until every base has advanced past) | **Vacuous** — no GC exists, no base registry to enforce against |
| 10 | Consolidator handover is an epoch; mode flip refused; migrate = disclosed wipe | **Enforced** (`consolidator_epoch.rs:36-109`) |
| 11 | One consolidator per file replica; byte-level file syncers out of contract | Unenforceable in code by nature; disclosed in Sync.md + README |
| 12 | Every field write resolves a cell backing (with disclosed carve-outs) | **Enforced & accurate** — Model.md right, Replication/Sync stale |

---

## 2. Verified-in-depth invariants (the 5 most load-bearing)

### 2.1 Invariants 2+3 — order ownership & intent vocabulary (PARTIAL)

What holds:
- `OrderKeyMinting` trait exists (`crates/holon-core/src/block_ordering.rs:221`); prod
  implementors: **only** `SqlBlockOperations` (`crates/holon/src/core/sql_block_operations.rs:255`).
- `LoroBlockOrdering` implements only `BlockOrdering` (`crates/holon-app/src/loro_seams.rs:292`);
  there is even a type-level witness module with a commented compile-fail proof
  (`loro_seams.rs:562-590`). Minting on the Loro path is genuinely unrepresentable.
- Archlint `order_minting` smell (`archlint/smells/order_minting.toml`) fences
  `gen_key_between`/`new_child_anchor` call sites; `sole_block_writer`
  (`archlint/smells/block_writes.toml`) fences raw block-table SQL to the four sanctioned
  writers.
- `holon_api::Block` carries **no** `sort_key` field (verified `crates/holon-api/src/block.rs:273-333`);
  the fi lives only on the boundary pair `SortedBlock` (`block.rs:1037-1046`). ADR 0005's
  core decision is realized.
- Org ingest intentionally never emits `sort_key` (`crates/holon-orgmode/src/block_params.rs:105-111`).

What does NOT hold:

1. **Live violation: GPUI board reorder mints in the frontend.**
   `frontends/gpui/src/render/builders/board.rs:261-263` calls
   `holon::storage::gen_key_between(prev, next)` and dispatches
   `set_field("sort_key", new_key)` — self-labelled "Known debt", ALLOW-suppressed.
   This is precisely the dual-writer/keyspace bug class Replication.md §5 declares
   "structurally impossible." In Loro mode this dispatch would hit the fail-loud Err
   (good); in SqlOnly it silently succeeds through the raw-SQL fall-through (bad).

2. **`set_field("sort_key")` "hard error" is mode-dependent runtime, not a type.**
   The Err lives at `crates/holon-loro/src/block_cell_registry.rs:322-338` — but
   `write_field` returns `Ok(false)` for `BackingSource::SqlOnly` at :218, after which
   `SqlBlockOperations::set_field` (`sql_block_operations.rs:834-880`) falls through to
   legacy raw SQL. So the Model.md invariant-12 phrasing "`set_field(\"sort_key\")` is a
   hard error" is true only in Full mode.

3. **`parent_id` is routed, not rejected** (`block_cell_registry.rs:291-299` →
   `backend.update_parent_id`). Replication §9.3's "set_field cannot carry
   sort_key/parent_id" is half-false as stated; routing-to-authority is the right
   behavior, but the doc should say "routed", not "cannot carry".

4. **The generic update-op vocabulary still accepts a `sort_key` key.** `sort_key` sits
   in `BLOCKS_KNOWN_COLUMNS` (`crates/holon/src/core/sql_operation_provider.rs:182`) and
   `RELOCATE_FIELDS` (`crates/holon-api/src/change_set.rs:57`); `decode_update`
   (`change_set.rs:275-295`) maps it to a `Relocate` and **discards the value**
   (ALLOW-tagged "until the Phase 5 flip"). Intent can still *carry* an order key; it is
   ignored rather than unrepresentable.

5. **`order_key_minter()` returns `Some(self)` unconditionally on the hybrid store**
   (`sql_block_operations.rs:213-214`); the Loro-mode guard is a runtime
   `matches!(self.consolidator(), Consolidator::Upstream)` **inside** `new_child_anchor`
   (:293-294) that returns `default_sort_key()`. The Option seam Model.md describes
   ("returns Some only for the store owner and None on the Loro path") is not what the
   code does — the mode check moved inside the body, and its Upstream branch returns a
   placeholder key instead of refusing.

**Type-level fix (parse, don't validate):** introduce a closed
`enum BlockWriteField { Content, Completed, Collapsed, ... }` — with **no** SortKey/ParentId
variants — parsed once at every intent boundary (`decode_update`, `dispatch_set_field`,
MCP `execute_operation`). Order changes become constructible only as
`Relocate { after_sibling }`. That deletes the string-match carve-outs in
`block_cell_registry.rs::write_field`, makes the board.rs debt a compile error, and makes
invariant 3 true by construction in both modes. Secondary: make `order_key_minter()`
mode-aware at wiring time (two store types, or `Option` resolved at DI), removing the
runtime Upstream branch.

### 2.2 Invariant 4 — sole writer & totality (PARTIAL: writer yes, totality no)

- Writer half: archlint-enforced (`sole_block_writer`, sanctioned writers =
  `sql_block_operations.rs`, `sql_operation_provider.rs`, `holon-loro/consolidator.rs`,
  `loro_sync_controller.rs` + test globs). Real, running, documented (Archlint.md:236,258).
- Totality half: **the silent `"A0"` fallback survives, relocated.** Replication.md §5
  cites it in `loro_sync_controller.rs`; it now lives in
  `crates/holon-loro/src/loro_backend.rs:884` (`.unwrap_or_else(default_sort_key)` in the
  settled-snapshot reader) and `:913` (`effective_sibling_sort_keys`). A Loro node with no
  fi silently projects the sentinel key instead of failing loud — the exact shape of the
  historical "sort_key stays A0" bug, and a direct violation of the project's
  fail-loud/never-fake rule. ADR 0005 also claims "`default_sort_key()` is removed" —
  false: `crates/holon-core/src/fractional_index.rs:33`, used in prod by
  `file_sync_controller.rs:678`, `reactive_view.rs:1949-1950`, `sql_block_operations.rs`.

**Type-level fix:** make the Loro fi read return `Result<MintedFi>` where
`MintedFi(String)` is a newtype constructible only by the order owner and by
`tree.fractional_index()`-present reads; the absent-fi case becomes an `Err` that the
snapshot loop must surface (or a disclosed re-mint through the owner). At minimum, swap
`unwrap_or_else(default_sort_key)` for a `bail!`/`tracing::error!` + PBT counter — the
R-1 debug_assert + NULL-count invariant mentioned in Replication §5 do not cover a
*sentinel* value, only NULL.

### 2.3 Invariant 8 — structural ops are commit points (TRUE for GPUI/web, FALSE for TUI)

- Mechanism verified: `dispatch_structural_as_commit_point`
  (`frontends/gpui/src/views/editor_view.rs:673-693`) chains
  `pending_commit_intent(live_text)` + the structural op through
  `dispatch_intent_chain` (`crates/holon-frontend/src/reactive.rs:2546-2562`), which
  awaits each intent sequentially in ONE task and aborts the chain on error — the
  "two fire-and-forget dispatches can reorder" failure is structurally excluded. All four
  GPUI structural entry points route through it (:941, :990, :1006, :1019). dioxus-web
  mirrors the pattern (`frontends/dioxus-web/src/editor.rs:280-356`).
- Contract is documented at the source: `editor_view_model.rs:274-281` — "MUST be
  dispatched ordered BEFORE the structural op ... or the pending text is lost."
- **Gap: the TUI dispatches `split_block`/`join_block` with no flush**
  (`frontends/tui/src/app_main.rs:1061,1088`). In SqlOnly (where pending editor state
  exists), the TUI can reproduce the canonical "Split position 8 exceeds content length 3"
  bug class today. The headless PBT mirror is legitimately exempt (per-keystroke Loro cell
  writes → no pending state), which also means **the ONE composed PBT cannot catch the
  TUI gap** — it never has divergent pending state on the SqlOnly+typing path the TUI
  takes.

**Type-level fix:** typestate the structural intents — make `SplitBlock`/`JoinBlock`
constructible only from a `CommitPoint` token produced by consuming
`EditorViewModel::pending_commit_intent` (or give `dispatch_intent_chain` a
`StructuralIntent` newtype whose only constructor takes `Option<CommitIntent>` first).
Any frontend then cannot dispatch a structural op without having answered the flush
question.

### 2.4 Invariant 1 — base diff, not cache (TRUE)

- `BaseStore`/`SyncBaseStore` exist (`crates/holon-filesystem/src/sync_base_store.rs`:
  `BaseKey{peer,file}` L42-45, trait L87-97, impl L107-110).
- Org diff reads "before" through the seam (`file_sync_controller.rs:621-637`);
  `LoroProjection::project` likewise (`loro_sync_controller.rs:376-385`, cold-boot
  fallback disclosed). Remaining `block_reader.get_blocks` uses are exactly the disclosed
  non-ancestor reads — including the exemplary comment at `file_sync_controller.rs:750-758`
  ("this read supplies only 'mine'").
- One stale doc detail: the module doc + Replication §3 say the impl "ignores the key /
  one global doc". False now — storage is a `HashMap<BaseKey, ...>` (`sync_base_store.rs:108`,
  lookups L175-194, isolation test L300); it's the *Loro consumer* that only ever uses
  `BaseKey::global()` while org uses per-file keys.

### 2.5 Invariants 10 + 12 — epoch guard & cell backings (TRUE; detail docs stale)

- `guard_consolidator_epoch` (`crates/holon-app/src/consolidator_epoch.rs:36-43`);
  mismatch without `HOLON_CONSOLIDATOR_MIGRATE=1` → `bail!` citing Model.md invariant 10
  verbatim (:99-109); with it → disclosed wipe-and-reseed (:86-96). Exactly as documented.
- All four backings of invariant 12 exist and are wired: `LoroTextCellBacking`
  (`loro_text_cell_backing.rs:45`), `LoroMetaCellBacking<T>` (`loro_meta_cell_backing.rs:95`,
  resolved `block_cell_registry.rs:609-642`), `LwwTextCellBacking` (`cell.rs:250`),
  `LwwScalarBacking<T>` (`cell.rs:301`, resolved `block_cell_registry.rs:547`);
  `sql_only_wired` at `block_cell_registry.rs:128`. The write_field carve-outs match the
  doc exactly (`_expected_*` :225-227; `id|depth|content_type|source_name` :240-242;
  unseeded-vault warn :255-265).
- Cell cache key `(EntityUri, String, TypeId)` confirmed (`cell_registry.rs:170`).
- ADR 0010 fully realized: `focused_block: Mutable<Option<EntityUri>>`
  (`reactive.rs:953`, init None :978); zero prod `editor_cursor`/`current_editor_focus`
  reads/writes; schema tombstone comment `crates/holon-turso/sql/schema/navigation.sql:17-18`.

---

## 3. Ranked findings

### F1 (HIGH) — Invariant 3 is a runtime convention with live leaks, not architecture

Evidence in §2.1: board.rs frontend minting (ALLOW-tagged), SqlOnly raw-SQL fall-through,
`sort_key` still a legal key in the update-op vocabulary (value silently discarded),
`parent_id` routed. The Model's strongest claim — "the original sin is structurally
impossible" (Replication §5) — is aspirational: it is *lint + one mode's runtime Err +
one discipline'd enum away* from true. Fix: closed `BlockWriteField` enum at the intent
boundary (§2.1). Until then, downgrade the doc phrasing from "cannot arise" to
"lint-fenced".

### F2 (HIGH) — Silent `A0` totality hole on the Loro fi read path

`loro_backend.rs:884,913` `.unwrap_or_else(default_sort_key)`. Silent sentinel data on
the exact field whose corruption motivated the whole Replication model. Violates
invariants 2+4 *and* the repo's own fail-loud philosophy (priority 4: "silently degrades
to look fine — never do this"). Also invalidates ADR 0005's "default_sort_key() is
removed" claim. Cheap fix now (error or disclosed re-mint), expensive bug later.

### F3 (MED-HIGH) — Cross-doc contradiction on cell backings (agents will rebuild existing code)

Replication.md:486-488 ("LoroMetaCellBacking ... LwwScalarBacking ... documented but not
implemented") and Sync.md:13,180 ("planned but not yet implemented") vs Model.md
invariant 12 and the code (both implemented + wired, §2.5). Model.md is right. An agent
loading Replication.md first would re-implement existing types or design around a
non-existent gap. One-line fixes in two docs.

### F4 (MED-HIGH) — Principles.md misclassifies `focused_block`; collides with ADR 0015's prerequisite

`Principles.md:394` lists `focused_block` as **per-VM per-instance widget state** (FU-1
pattern); `UI.md:12` + ADR 0010 define it as a **window-global `UiState` singleton**
("per-instance homes would let two editors both believe they hold focus"). Direct
contradiction on the exact primitive that ADR 0015 rule 5 says must be re-keyed by
`(id, occurrence)` before P2 — whoever executes that work will read one of these two
docs. Code agrees with UI.md (`reactive.rs:953`).

### F5 (MED) — Invariant 8 unenforced for TUI; unenforceable by construction for new frontends

§2.3. TUI `app_main.rs:1061,1088` dispatches structural ops flush-free; nothing but
convention makes the next frontend (Blinc, mobile) use
`dispatch_structural_as_commit_point`. Typestate fix in §2.3. Also note: the composed
keystone PBT structurally can't catch this gap (headless mirror has no pending state on
that path) — per CLAUDE.md rule 2, that's a "make E2E more like prod" candidate.

### F6 (MED) — Invariant 9 (tombstones) is vacuous

Repo-wide: no tombstone GC, no retention logic, and — the deeper issue — **no registry of
replica bases** that a future GC could consult. The invariant is currently unfalsifiable:
nothing can violate it because nothing collects, but nothing could enforce it either.
Either mark it explicitly as a *design constraint on future GC* (not an invariant of the
running system), or land the minimal representable artifact now: a
`ReplicaBaseRegistry` (even just the set of `BaseKey`s + their watermarks) so a GC has
something to be checked against — plus a PBT invariant the day GC exists.

### F7 (MED) — ADR anchor rot after the PBT endgame and refactors

- ADR 0012 ("Accepted ... implemented and load-bearing") cites `E2ESut` and
  `sut_capabilities.rs` blanket impls — **both deleted** in the 2026-07-05 PBT endgame
  (only handoff .md fossils remain; `SutHandle` moved to `transition_dispatch.rs:168`
  from the cited :158). The capability contract itself survives (capabilities.rs,
  ComposedSut), but every concrete pointer in its §3 is dead.
- ADR 0005 status "Proposed" though its core decision shipped (Block field removed);
  its claims "default_sort_key() removed" (false, F2) and `children_of_window` (never
  implemented — docs-only) need a Resolved/Errata block.
- ADR 0010 status "Proposed" though fully implemented and schema-tombstoned.

### F8 (LOW-MED) — Mode-axes framing: 4 declared, ~2 actually free; missing the epoch dimension

Assessment of Model.md's "Four orthogonal mode axes":
- **Merge fidelity is not an axis** — Model.md itself *derives* the ladder from store
  presence ("this asymmetry derives the rule"). It's a dependent variable; listing it as
  an orthogonal axis contradicts the doc's own derivation two sections later.
- **Transport is not independently flippable today** — Sync.md's own component table:
  Iroh "(bundled with Loro, future: separate)". Aspirationally an axis, actually a rider.
- So the real grid today is 2x2 (Loro store x org adapter) — exactly Sync.md's four-row
  combination table. The doc's honesty note ("the switch flips several axes at once")
  under-states it: two of the four axes aren't independently reachable at all.
- **What the grid misses:**
  (a) the **epoch/path dimension** — invariant 10 says the dangerous object is not a
  *point* in the grid but a *move* between points; the axes give no vocabulary for
  "which consolidator minted the current bases", which is the thing the startup guard
  actually checks;
  (b) **per-entity-type authority** — the grid is block-only; external replicas
  (Todoist/JIRA, Principles.md "Authority by Data Type") and per-sibling-set foreign
  order owners (Replication §5 mixed-origin decision) live outside it;
  (c) enforcement reality is also **frontend-dependent** (F5), an axis the model
  deliberately excludes from modes but which currently changes which invariants hold.
- Suggested refresh: mark fidelity/transport as *derived* rider rows, add one line naming
  the epoch as the fifth (path-, not point-) dimension, and scope the grid explicitly to
  the block replica set.

### F9 (LOW) — Framing tension: "durable change log" vs "convergent state, ephemeral projection"

Principles.md Layer 1 calls "Loro oplog + Turso CDC + WAL" a "durable, ordered,
identity-bearing record of every change" and Layer 3 Turso "query-shaped, durable";
Model.md layer 4 says "convergent state, not an event log" and layer 3 "ephemeral by
contract". ADR 0015's P4 analysis sides decisively with Model.md (CDC "must not" be
treated as durable history; `EventRing` cap-4096 eviction → forced resync). Principles.md's
Layer-1 box is the one place an agent could still pick up the wrong contract.

### F10 (LOW) — Residual staleness worth a sweep

- ADR 0001 still names the project "Rusty Knowledge"; its Turso-cache-centric framing
  predates the 2026-05 authority flip; Accepted with no supersession pointer to
  Replication.md/Model.md. Add a header note.
- Replication.md micro-rot: A0-fallback file citation (moved to `loro_backend.rs`, F2);
  "impl ignores the key" (§2.4); "LoroTreeParentCellBacking/LoroTreePositionCellBacking
  not implemented" is still *correct* — keep that half.
- Model.md invariant 12's "`set_field(\"sort_key\")` is a hard error" should say "hard
  error in Full mode; SqlOnly routes to the SQL order owner" (§2.1 item 2).
- Model.md layer-2 cell "mints every fractional index" overstates vs Replication §5's
  per-sibling-set/foreign-owner nuance (Todoist rows get fi from their projector).

### Explicitly NOT findings (verified sound)

- **The Petri-net engine refactor does NOT stale Model.md/UI.md/RenderPipeline.md.**
  The 7-parent merge `30127e8e12` bundles the *task-ranking* engine
  (`crates/holon-engine` YAML/Rhai/WSJF + `crates/holon-petri` materialization) — it is
  not a frontend replacement. The "old reactive types removed" were pre-`ReactiveEngine`
  plumbing (`AppState`, `CdcState`, `BlockWatchRegistry`); `ReactiveEngine`
  (`reactive.rs:1165`), `ReactiveViewModel` (`reactive_view_model.rs`), `ReactiveView`,
  and `LiveData<Block>` (`event_infra_module.rs:87`) are all alive. UI.md:37's
  "IMPLEMENTED" claim still matches code.
- Invariant 7 is true by construction (`TransientTextMergeProvider` exists; Sync.md
  SqlOnly base-3-way text merge documented with disclosure).
- Invariant 1, 10, 12, ADR 0010, cell key, archlint gates, "holon-markdown unwired"
  (workspace member, zero dependents): all verified TRUE.
- ADR 0015 (fresh, 2026-07-06) is unusually rigorous — it self-audits against Model.md
  invariants (incl. flagging the inv-4 "per store, not per table" ambiguity) and
  distinguishes prediction from proof. Use it as the template for future ADRs.

---

## 4. Suggested actions (ranked, smallest-first within rank)

1. **Kill the silent A0 fallback** (`loro_backend.rs:884,913`) → error or disclosed
   re-mint through the owner (F2). ~1 session incl. PBT sentinel-value invariant.
2. **Two-line doc fixes**: Replication.md:486-488 + Sync.md:13,180 backing status (F3);
   Principles.md:394 `focused_block` reclassification (F4).
3. **`BlockWriteField` closed enum at the intent boundary** (F1) — deletes the
   string-matched carve-outs, makes board.rs debt a compile error. Pairs naturally with
   the already-planned "Phase 5 flip" that retires `sort_key` from `RELOCATE_FIELDS`.
4. **TUI commit-point flush + typestate for structural intents** (F5); consider a PBT
   config that exercises SqlOnly pending-state via the TUI-shaped path.
5. **ADR hygiene pass**: statuses (0005 → Accepted+errata, 0010 → Accepted), ADR 0012
   anchor refresh post-endgame, ADR 0001 supersession header (F7, F10).
6. **Model.md mode-axes refresh**: derived-axis annotations + epoch dimension +
   block-scope note (F8); invariant-12 mode nuance; layer-2 minting nuance (F10).
7. **Invariant 9**: either re-label as a GC design constraint or land the minimal
   `ReplicaBaseRegistry` representable artifact (F6).
