# Holon Vision Review

Independent reading of the vision against the architecture docs and the code.
Baseline: `main` = `4e2ee368` (2026-09-03). Read-only review. Every code claim
names a file and a line I opened. Where I could not verify, I say so.

Reading order used: `docs/Vision.md`, `docs/Vision/*.md`, `docs/Strategy/*`,
`docs/Proposals/VisionGapAnalysis-2026-07-11.md`, `docs/Architecture.md` and
all of `docs/Architecture/*.md`, ADRs 0024, 0026, 0028–0034, the vault altitude
docs under `holon-pkm/Projects/Holon/`, then targeted code checks.

---

## 1. The vision in one page

**What Holon is for.** Holon is a trust system that uses productivity data.
Its purpose is not throughput. It is to let one person reach flow because they
trust three things: nothing is forgotten, the next task is the right one, and
the context for that task is one gesture away
(`docs/Vision.md` §Core Purpose; `docs/Vision/LongTerm.md` §The Flow State Goal).

**Who the user is.** The near-term user is Martin, and through read-only
sharing his wife (`holon-pkm/Projects/Holon/MVP Definition.org` §Goal). The
target user is a power knowledge worker whose tasks, notes, mail, calendar and
files live in five tools. Beginners are an explicit non-goal
(`docs/Strategy/Goals.md` §Non-Goals).

**The load-bearing promises.** I count eight. Each later section grades the
architecture against these.

| # | Promise | Where it is stated |
|---|---|---|
| P1 | **Nothing forgotten.** A Watcher sees every system and surfaces what you miss, including stale knowledge (freshness metadata). | `Vision.md` §Core Purpose, §3.1; `Vision/AI.md` §The Watcher |
| P2 | **The right next thing.** Ranking is derived from a Petri net over real tokens and an objective function stored as data, never from a hand-typed priority alone. | `Vision/PetriNet.md` §Five Primitives, §WSJF |
| P3 | **Context is one gesture away.** Context bundles assemble every related item across systems, as one view, where the source system is metadata. | `Vision.md` §1.1; `Architecture/Principles.md` §Context Bundles |
| P4 | **External systems are first-class and integration comes first.** Zero migration. Todoist, JIRA, mail and calendar are replicas with bidirectional sync, embedded anywhere, operated from Holon. | `Vision.md` §1; `Vision/PetriNet.md` §Integration-First; `Strategy/FIRST_RELEASE_FEATURES.md` |
| P5 | **Local-first, plain-text durable, multi-device, shareable.** CRDT for owned data, org files as a human-readable replica, P2P sync with no server, sharing of subtrees. | `Vision.md` §4, §5; `Architecture/Model.md` |
| P6 | **Structural primacy.** Intelligence lives in schemas, typed edges, matviews and queries. AI is optional and must pass the substitution test. | `Vision.md` §Trade-Offs (6); `Vision/AI.md` §Structural Primacy |
| P7 | **Everything is runtime data.** Rules, render profiles, perspectives, the objective function, AI personalities and connectors are vault data, never Rust per feature. | `Proposals/VisionGapAnalysis-2026-07-11.md` §1; ADR 0034 §1 |
| P8 | **Human authority over machine initiative.** The Integrator proposes, the human confirms at System-1 speed; agents earn autonomy per transition; every agent write carries provenance and can be supervised as a query. | `Vision/AI.md` §The Integrator, §Trust Ladder; `Vision/UnifiedHumanAgentManagement.md` §Gaps |

Two secondary promises sit underneath: a calm three-mode UI (Capture, Orient,
Flow; `Vision/UI.md`) and cross-platform reach including mobile and web
(`Vision.md` §8).

**The one sentence the docs converge on.** Holon is one logical block tree,
replicated across heterogeneous partial replicas, consolidated by one merger,
and projected one way into an incremental query pipeline
(`docs/Architecture/Model.md` §One sentence). That sentence is the architecture
answer to P5 and P7. P1 to P4 and P8 are what still has to be built on top of it.

---

## 2. Architecture as documented versus as built

Status legend: **matches**, **partial**, **contradicts**, **undocumented-in-code**
(the doc describes something with no code home), **doc-stale** (the code has
moved past the doc).

| # | Claim | Doc anchor | Code anchor | Status | Note |
|---|---|---|---|---|---|
| A1 | One dispatcher with three gates before the provider: authorization, declared guards, net guard. | ADR 0032 §3; `Architecture/Model.md` §Five layers | `crates/holon/src/api/operation_dispatcher.rs:58-63` (fields), `:386` (`enforce_boundary`), `:430` (`enforce_guard`), `:479` (`enforce_net_guard`), `:510` (`assert_net_guard_installed`) | matches | The ADR 0032 "third gate" concern is resolved in code exactly as the ADR asked. |
| A2 | Net guard trait, inert default, placement policy. | ADR 0032 §3 | `crates/holon/src/api/net_guard.rs:168` (`trait NetGuard`), `:179` (`InertNetGuard`), `:23-30` (`RefusalClass`) | matches | Confirmation classes exclude `Authorization` by construction. |
| A3 | Derived Petri-net projection with conflict and cycle analysis. | ADR 0032 §2; vault `Petri Net Execution.org` says "TODO Derive the net projection" | `crates/holon-net/src/lib.rs:1-30` (modules `analysis`, `compile`, `bridge`, `net`); workspace member `Cargo.toml:11` | partial / vault-stale | The crate exists on `main`. Whether rule-save runs the cycle check is not verified here. The vault tracker still lists the piece as TODO. |
| A4 | Operation descriptors carry a data guard and a marking delta. | ADR 0031; ADR 0032 §Consequences (1) | `crates/holon-api/src/render_types.rs:424` (`guard: OpGuard`), `:446` (`marking_delta: MarkingDelta`) | matches | |
| A5 | Exactly one writer per store; the projection is total. | `Model.md` invariant 4; `Replication.md` §9 | `archlint/smells/block_writes.toml` (sole-writer smell); `crates/holon-loro/src/block_cell_registry.rs:355-357` and `:618-620`, `:729` (disclosed SQL fallbacks for unseeded blocks) | partial | The smell sanctions two mode-dependent writers. The cell-mode text path can still write SQL directly when Loro has no node, with a WARN. This is the "second-writer hole" the readonly lane is closing. |
| A6 | Write tier: read-only formats refuse write-back, default `ReadOnly`, enforced at the dispatcher. | ADR 0034 §6 | `crates/holon-core/src/file_format.rs:304-310` (enum); enforcement only in `crates/holon-filesystem/src/file_sync_controller.rs:1505`, `:4296`, `:5455`, `:6761`; nothing in `operation_dispatcher.rs` on `main` | partial | Dispatcher-level `WriteTierAuthority` exists only in the uncommitted lane `.claude/worktrees/readonly-edits` (`operation_dispatcher.rs:65`, `:179`; `block_cell_registry.rs:65-114`). |
| A7 | Typed rows as JSON Lines are the neutral contract; one sink. | ADR 0034 §2 | `crates/holon-rows/src/lib.rs:1-15`; `crates/holon-core/src/file_format.rs:53` (`TypedRowSet`); `crates/holon/src/core/typed_row_sink.rs:34`, `:126` | matches | Landed as commit `73ad32ba`. |
| A8 | A wasm plugin host on `wasmi` runs format guests. | ADR 0034 §3 | Not on `main`: no `crates/holon-plugin-host`, no `wasmi` in root `Cargo.toml`. Present only in lane `.claude/worktrees/lowcode-inc2a/crates/holon-plugin-host/src/lib.rs:1-25` | undocumented-in-code (on main) | The lane's crate matches the ADR's shape: empty linker, pure guests, five-function ABI. |
| A9 | `jaq` is the single mapping language; sidecars carry a verbatim UTCP manual. | ADR 0034 §4, §5 | No `jaq` in any `Cargo.toml` on `main` or in the lowcode lanes. UTCP appears only in test files of lane `lowcode-inc4` (`crates/holon-mcp-client/tests/utcp_manual_roundtrip.rs`, `sidecar_conformance.rs`) | undocumented-in-code | The mapping half of ADR 0034 is not started on `main`. |
| A10 | The bespoke kitchen parser and shopping client are deleted once the plugin path lands. | ADR 0034 §Consequences | `crates/holon-kitchen/src/{cook.rs,shopping.rs,shopping_sync.rs}` present; wired at `crates/holon-app/src/wiring.rs:360-364` (`CookFormatAdapter` in the `FormatRegistry`) | partial | Expected while Inc 2a/4 are in flight. The risk is the old path outliving the new one. |
| A11 | `FileFormatAdapter` has one production impl (org); markdown was removed 2026-07-06. | `Model.md:45`; `Sync.md:471` | Four impls: `crates/holon-orgmode/src/file_format.rs:44`, `crates/holon-kitchen/src/file_format.rs:34`, `crates/holon-markdown/src/logseq.rs:81`, `crates/holon-markdown/src/obsidian.rs:121`. Markdown deliberately unwired: `wiring.rs:356-358` (both claim `md`, ruling D56.a) | doc-stale | `holon-markdown` was re-added as read-only Tier R/O (`Architecture/CrateMap.md` row). Model.md and Sync.md still say removed. |
| A12 | Loro defaults OFF; `HOLON_CRDT_ENABLED` switches it on. | `Sync.md:93` (env table) | `crates/holon-frontend/src/config.rs:564-566` (`unwrap_or(true)`); `crates/holon-app/src/wiring.rs:174`, `:225` | doc-stale | Model.md already says default ON (ruling D69.a). Sync.md contradicts Model.md. |
| A13 | CrateMap is generated and validated against the tree. | `Architecture.md` §Crate Responsibilities; `CrateMap.md` header | Workspace members absent from CrateMap: `holon-capability` (`Cargo.toml:6`), `holon-net` (`:11`), `holon-logseq-db` (`:16`), `holon-kitchen` (`:20`), `holon-rows` (`:21`) | doc-stale | Either `just arch-validate` is not in the landing gate, or the baseline was updated without regenerating the map. |
| A14 | Schema module registry has ten modules. | `Schema.md:43-54` | `crates/holon-turso/src/schema_modules.rs` has seventeen `impl SchemaModule` (`:91`, `:338`, `:557`, `:616`, `:666`, `:700`, `:740`, `:872`, `:901`, `:989`, `:1028`, `:1094`, `:1131`, `:1166`, `:1200`, `:1257`, `:1334`); tables `block_contributes_to`, `advice_suppressed`, `block_redirects` owned at `:330-348` | doc-stale | Missing from the doc: TrustProposals, IntegrationState, History, BlockDerived, AutomationsJournal, JournalDayPages, JournalFeed. |
| A15 | Undo is in-memory only; the persistent log is write-only. | `Operations.md` §OperationLogStore ("write-only today") | `crates/holon-core/src/undo.rs:39` (`trait UndoStore`); `crates/holon/src/api/undo_persistence.rs:54` (`impl UndoStore for SqlUndoStore`) | doc-stale | ADR 0032 §Concerns (3) already measured this. Operations.md was not updated. |
| A16 | Entity identity has one minting authority, witness types, a lint, and Model.md invariant 13. | ADR 0029 D1–D3, §Enforcement | Witness types: `crates/holon-api/src/identity_minting.rs:1-40`. Lint: `archlint/smells/` holds eight files, none named `identity_minting`. Invariant 13: absent from `Model.md` | partial | The ADR says "a decision without a lint is not a decision". The lint and the Model.md line are still owed. |
| A17 | Own-device pairing: whole store, device-local layout doc outside the registry, `replicate_all` over iroh, capability at one acceptor, refuse on mounts. | ADR 0033 §1–§5 | `crates/holon-loro/src/loro_document_store.rs:34` (`DocScope`), `:66`, `:83-84` (`layout_doc`, `holon_layout.loro`); `crates/holon-loro/src/container_registry.rs:236` (`replicate_all`); `crates/holon-sharing/src/policy.rs:61` (`Capability::Write`); `crates/holon-sharing/src/acceptor.rs:145` (`admit`); `crates/holon-loro/src/device_pairing_op.rs:260-271` (`pair_offer`, `pair_cancel`, `pair_with_owner`); `crates/holon-integration-tests/src/pbt/composed/two_instance_transport.rs` (both legs) | matches | §6 archive-and-re-import: the archive step is referenced in `device_pairing.rs`; I did not verify the re-import leg. Memory says it is queued. |
| A18 | Sharing is a policy overlay; increments 4–6 exist; end-to-end does not ship. | ADR 0028; `FeatureMap.md` §Sharing | `crates/holon-sharing/src/{policy.rs,log.rs,alias_ledger.rs,acceptor.rs}`; hostile-envelope PBT landed `4a86161b` | partial | Consistent with the docs. Read-only mount for a third party (vault AC-4) is still TODO in `Engine Foundations.org:684`. |
| A19 | Trust gate coerces sub-threshold dispatches into proposals. | `VisionGapAnalysis` §3 C5; ADR 0024 P4 | `crates/holon-profiles/src/trust.rs:116` (`TrustPolicy`); `crates/holon/src/api/operation_engine.rs:2285` (`coerce_to_proposal`); `crates/holon-turso/sql/schema/trust_proposals_matview.sql` | partial | Default policy is trust-all, so the gate is inert until a vault policy exists. No UI. |
| A20 | Provenance and history are a queryable relation; the automations journal is a query. | ADR 0024 P8 | `crates/holon-api/src/history.rs:298` (`trait HistoryStore`); `crates/holon-turso/sql/schema/history.sql`, `automations_journal_matview.sql:1-17`; `crates/holon/src/api/holon_rule_watcher.rs:454` (`OpOrigin::Rule`) | matches | This is the substrate the Guide and agent supervision need. |
| A21 | Time is data: clock relation, grains, recurrence. | ADR 0024 P5 | `crates/holon-api/src/clock.rs:130` (`Grain`), `:206` (`Recurrence`); `crates/holon-turso/sql/schema/clock.sql` | matches | |
| A22 | One rule language; `action_watcher` is re-understood and deleted. | ADR 0024 §Consequences, Phase 3 | Both `crates/holon/src/api/action_watcher.rs` and `holon_rule_watcher.rs` exist; `LegacyAction` in `crates/holon-api/src/types.rs` | partial | Two rule machines coexist. `Operations.md` §Query-Triggered Actions still documents the legacy one as current. |
| A23 | Perspectives as data with an active-perspective pointer. | `VisionGapAnalysis` C8 | `crates/holon-api/src/perspective.rs:132` (`PerspectiveSpec`); `crates/holon-loro-wiring/src/loro_ui_watcher.rs:494` | matches | |
| A24 | Full-text search through the Turso fork; unified search UI. | `VisionGapAnalysis` C3; `FIRST_RELEASE_FEATURES.md` F7 | `Cargo.toml:217-222` (fork features `fts`, cfg-gated off on wasm); `crates/holon-turso/src/engine_functions.rs:58`, `:65` (`fts_match`, `fts_score`); `crates/holon/src/api/query_engine.rs:99` (`quick_open_search`); `frontends/gpui/src/search_ui.rs:1-11` | partial | Bugfunnel entry `2026-09-03-quick-open-search-returns-no-matches-for-every-query.md` is still `status: OPEN` on `main`. The fix is not on `4e2ee368`. No embeddings anywhere. |
| A25 | Task syntax parser, WSJF ranking, MCP task pull. | `Vision/PetriNet.md` §Task Syntax, §WSJF; vault `Now.org` | `crates/holon-petri/src/parser.rs:14`, `:44`, `:314` (`@[[Person]]:` delegation); `crates/holon-petri/src/lib.rs:978` (`materialize`), `:1452` (`rank_tasks`); `frontends/mcp/src/tools.rs:1664` (`now_for_agent`), `:1708` (`claim_task`), `:1951` (`complete_task`) | partial | FeatureMap §Unpinned: task ranking is exercised by nothing in the keystone. Vault `Engine Foundations.org:627`: verb dictionary TODO. |
| A26 | The standalone engine is a YAML simulator fed from the native catalog. | `Engine.md`; ADR 0031 | `crates/holon-engine/src/{engine.rs,guard.rs,objective.rs,yaml/}` | matches | "Fed from the catalog" is a stated direction; I found no loader from `OperationDescriptor` into the engine. |
| A27 | The three AI services are declarative Petri-net configuration. | `Principles.md:177-197` ("target architecture — not yet implemented") | No Watcher, Integrator or Guide code (grep hits are unrelated words) | matches (doc is honest) | See §3 G3. |
| A28 | Holon hosts MCP Apps in sandboxed iframes; the Dioxus web frontend is primary. | `Integrations.md:7-127`; `Vision.md:207` | No `AppBridge`; `frontends/dioxus-web` parked (vault `Multi-Frontend Strategy.org:60` WONT); primary is GPUI (`Engine.md:59-61`) | contradicts | Three vision docs name three different primary UIs: Dioxus web (`Vision.md:207`), Flutter (`LongTerm.md:436`, `Goals.md:109`), GPUI (`FIRST_RELEASE_FEATURES.md:127`, vault). |
| A29 | Todoist is implemented with full CRUD; the reference connector uses REST and OAuth2. | `Goals.md:29`; `FeatureMap.md` §Integrations | `holon-todoist` crate deleted (`Replication.md` §1); `assets/integrations/todoist.yaml:27-33` is MCP-over-HTTP with a static token, `oauth: false`; bundled at `crates/holon-mcp-client/src/bundled_sidecars.rs:40-47` | doc-stale | FeatureMap's "REST transport, OAuth2" does not match the sidecar. Nothing in the keystone exercises sync or write-back. |
| A30 | Cross-system entity identity: canonical entities, aliases, proposals. | `Vision.md` §1.0; `Schema.md:163-165` | `crates/holon/src/identity/provider.rs:48` (`IdentityProvider`), `:61` (`merge_entities`), `:255` (`propose_merge`); `crates/holon-turso/sql/schema/identity.sql` | partial | Operations exist, tables empty by default, no resolver, no confirmation stream. Vault `Entity Identity.org:50` defers to G2. |
| A31 | Compass layer: mission, problems, goals as typed blocks with contributes-to edges and review cadence. | `LongTerm.md` §Compass Layer | `assets/default/Compass.org:1-60` (templates with `:compass:`, `:contributes-to:`, `:review-cadence:`); `crates/holon-turso/sql/schema/block_contributes_to.sql:1-12` | partial | The data shape exists. No query or rule consumes `review-cadence` yet. |
| A32 | Attention environment: desk document, zones, zoom levels, shell trait. | ADR 0026 (Proposed) | No `desk`, `zoom_level` or `trait Shell` in the tree | undocumented-in-code | A complete design with zero code. Consistent with ADR status Proposed. |
| A33 | The three modes ship as default layouts and profiles. | `Principles.md` §The Three Modes | No capture overlay, orient view or flow shell found; leader-key chords exist (`assets/default/keybindings.yaml:1-14`) | partial | Vault `MVP Definition.org` §Goal marks three modes out of G1 scope. Principles.md reads as if they exist. |
| A34 | Degraded signal bus is wired in the shipped GPUI container. | vault `Cross-Cutting Concerns.org:138` (TODO) | `crates/holon-loro/src/degraded_signal_bus.rs:269`; `frontends/gpui/src/main.rs:166-167`, `:268` | vault-stale | Wired. "Absence is a hard boot error" not verified. |
| A35 | Kanban and calendar render expressions. | `FIRST_RELEASE_FEATURES.md` F8, F9 | `crates/holon-frontend/src/shadow_builders/board.rs`, `board_lane.rs` exist; no `calendar` builder | partial | |
| A36 | Offline command log for external systems. | `Storage.md` §Command Sourcing ("nothing implemented, by design") | Nothing | matches (honest) | |
| A37 | Windows is a target platform. | `Vision.md` §8 | `Storage.md` §Database Access: "Windows: unsupported — `open_database` returns an error" | contradicts | The vision promises what the storage layer refuses. |
| A38 | Keystone PBT and bug funnel are the quality gates. | `CLAUDE.md`; `FeatureMap.md` | `crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`; bug funnel 636 entries, 265 `OPEN` (`scripts/bugfunnel.py counts`) | matches | The funnel's own numbers: ENVIRONMENT 38 %, COVERAGE 30 %, ORACLE 20 %, PERCEPTION 13 %. |

---

## 3. Gaps between vision and architecture

Ranked by how load-bearing they are for the eight promises in §1. "Home"
means a named place in the architecture where the capability would land.

### G1. External systems as replicas with intent and write-back (P4)

The vision's first promise. `Model.md` layer 1 reserves the slot and ADR 0034
gives connectors a shape, but on `main` every external system is read-only
by default (`file_format.rs:304-310`), the write-back lease taxonomy of ADR 0024
P4 has no code, and the reference connector is unpinned (`FeatureMap.md`
§Unpinned "Todoist"). Kitchen and shopping proved the read leg and a write leg
for an id-less list, in bespoke Rust the ADR marks for deletion. What has no
home yet:

- Leased external effects (exactly-once promotion to Todoist): ADR 0032 §5 says
  "not in the first version".
- The offline command log (`Storage.md` §Command Sourcing).
- Conflict UI and sync-status indicators (`Principles.md` §Sync Status).

### G2. Entity identity resolution, the Zeroth Principle (P3, P4, P8)

Without canonical Person, Organization and Project entities there are no
cross-system edges, and without edges the context bundle is a search result,
not a graph. What exists is a seam: three empty tables and merge/propose
operations (`identity/provider.rs:61`, `:255`). The parser already recognizes
`@[[Person]]:` (`holon-petri/src/parser.rs:44`) but binds it to nothing durable.
The vault defers this to G2 (`Entity Identity.org:50`), which is defensible for
a single-source vault and fatal for P3 the day a second source arrives.

### G3. The three AI roles as authored rule packs (P1, P6, P7)

The 2026-07 gap analysis predicted that after C2 (history), C3 (search), C5
(trust) and C6 (clock) the Watcher, Integrator and Guide become vault data with
no personality-specific Rust. Those four primitives have all landed (§2 A19,
A20, A21, A24). Nobody has written the rule packs. The remaining architecture
holes are:

- ADR 0015 display placement is Proposed and unimplemented (`FeatureMap.md`
  §Unpinned), and the Watcher's output model in ADR 0024 §Advice is emission
  into display placement.
- No Orient surface exists to show emitted advice (A33).
- Freshness (`review-cadence`) exists as a property (A31) with no consumer.

### G4. The attention environment and the three modes (P1, secondary UI promise)

ADR 0026 designs the desk, zoom levels and OS shell. Nothing is built (A32). The
three modes are documented in `Principles.md` as shipped defaults (A33) and in
the vault as out of G1 scope. Two docs disagree about whether this is present
tense.

### G5. Unified human and agent management (P8)

The landscape review names the killer demo: agent provenance plus re-executable
source blocks plus live supervision (`UnifiedHumanAgentManagement.md` §What
would move the needle most). Provenance (A20), the trust gate (A19), MCP claim
and release (A25) and `execute_source_block` (`frontends/mcp/src/tools.rs:866`)
all exist. Missing: the sessions-under-topics view, the open-questions inbox,
the "agents needing me" view (vault `Dogfooding & Agents.org:39-56`), and a
supervision query surfaced anywhere in a UI.

### G6. Petri-net execution beyond the guard (P2)

Landed: declared marking deltas, the net guard, the derived projection crate.
Not landed: claim disciplines and leases (ADR 0032 §5), undo re-founded on
deltas (§7), the editing lease, the Self digital twin, the objective function
as prototype blocks consumed by a live path, and any deliberation over a live
vault (vault `Engine Foundations.org:702-729`). `holon-engine` is still a YAML
simulator with no loader from the native catalog (A26).

### G7. Semantic search and embeddings (P3)

FTS exists in the Turso fork and is compiled out on wasm (`Cargo.toml:215-216`).
No embedding, no vector index, no `similar()` function. The vision names
Tantivy and sentence-transformers (`Vision.md:219`, `:328`); neither is in the
tree, and the ruling of 2026-07-11 chose Turso FTS instead. The vision text was
never updated.

### G8. Multi-frontend, web and mobile (secondary promise)

GPUI is primary; Dioxus web is parked; MCP Apps hosting was premised on the
browser and has no host (A28). Android and iOS have no keystone coverage
(`FeatureMap.md` §Unpinned). Windows is unsupported by the storage layer (A37).

### G9. Third-party sharing end to end (P5)

Pairing between own devices landed. Read-only sharing with another person
(vault AC-4) is TODO (`Engine Foundations.org:684`). `BlobSig` is unkeyed and
deferred (ADR 0033 §7).

### G10. Cross-device undo and container-scoped journals (P8)

ADR 0032 §7 requires the occurrence journal to be container-scoped and
syncable. `UndoStore` is per replica (`undo.rs:39`). Sharing-aware undo has no
increment scheduled.

---

## 4. Doc staleness list

Statements the code has moved past, with the code evidence. Ordered by how
likely a fresh agent is to be misled.

1. **Three primary frontends in three docs.** `Vision.md:207` (Dioxus web
   primary), `LongTerm.md:436` and `Goals.md:109` (Flutter), versus
   `Engine.md:59-61` and vault `Multi-Frontend Strategy.org:31` (GPUI).
   `frontends/flutter/rust_builder` and `frontends/dioxus/assets` are remnants.
2. **`holon-markdown` "removed 2026-07-06".** `Model.md:45`, `Sync.md:471`.
   The crate is a workspace member (`Cargo.toml:19`) with two adapters
   (`logseq.rs:81`, `obsidian.rs:121`), unwired by ruling D56.a
   (`wiring.rs:356-358`).
3. **Loro default OFF.** `Sync.md:93`. Default is ON (`config.rs:564-566`).
4. **CrateMap missing five crates.** `Architecture/CrateMap.md` versus
   `Cargo.toml:6,11,16,20,21`. The doc claims machine validation.
5. **Schema.md lists ten modules; the code has seventeen.** `Schema.md:43-54`
   versus `schema_modules.rs:91-1334`.
6. **Undo is in-memory only.** `Operations.md` §OperationLogStore. `UndoStore`
   is persisted (`undo_persistence.rs:54`).
7. **Todoist implemented with full CRUD / REST + OAuth2.** `Goals.md:29`;
   `FeatureMap.md` §Integrations. The sidecar is MCP over HTTP with a static
   token (`todoist.yaml:27-33`), and nothing pins it.
8. **Tantivy and sentence-transformers.** `Vision.md:219`, `:328`;
   `LongTerm.md:440`. Ruled out 2026-07-11 in favour of Turso FTS
   (`VisionGapAnalysis` §4 Increment 2).
9. **Query-actions documented as current.** `Operations.md` §Query-Triggered
   Actions. ADR 0024 supersedes them with `holon_rule`; both watchers still
   exist (`action_watcher.rs`, `holon_rule_watcher.rs`).
10. **ADR 0029 enforcement described as required, not landed.** No
    `archlint/smells/identity_minting.toml`; no invariant 13 in `Model.md`.
11. **Vault `Petri Net Execution.org:35` "TODO derive the net projection".**
    `crates/holon-net` exists on `main`.
12. **Vault `Cross-Cutting Concerns.org:138` "TODO wire DegradedSignalBus".**
    Wired at `frontends/gpui/src/main.rs:166`.
13. **`Principles.md` §The Three Modes** reads as shipped defaults. No mode
    surfaces exist; the vault marks them out of G1.
14. **`Strategy/MVPs.md`** describes `RenderEngine.query_and_watch`, a TUI
    with `ViewMode`, and a `holon-todoist` crate. All three are gone. The doc
    is a 2025-era design note filed as strategy.
15. **`Integrations.md` §MCP Apps** is a target that depends on a parked
    frontend. The banner says target; the length says shipped.
16. **`Vision.md` §8** promises Windows; `Storage.md` says unsupported.
17. **`Replication.md` §1** says "no Todoist integration is currently wired".
    A Todoist MCP sidecar is bundled (`bundled_sidecars.rs:46`), though off
    unless enabled.

---

## 5. Ideas: what I would do differently or earlier

Each idea carries its first-principles reason.

**I1. Ship the first Watcher as vault rules this month, before more sync
surface.** First principle: the vision's differentiator is P1, and every
primitive it needs has landed (history, clock, trust gate, FTS, compass
properties). A Watcher that emits "review overdue", "deadline within buffer",
"delegated and silent for N days" is three `holon_rule` blocks and one Orient
page. No Rust. It also validates P7 for real: if the rules need engine changes,
that is the finding the substitution test exists to produce.

**I2. Make generated docs fail the landing gate.** First principle: a generated
doc that drifts is worse than no doc, because it carries authority it no longer
has. CrateMap and Schema.md drifted while claiming validation. Add
`just arch-validate` and `scripts/featuremap.py check` to the per-land gate, and
generate the schema module table from `schema_modules.rs`.

**I3. Retire the old path the moment the new one lands, per crate.** First
principle from `CLAUDE.md`: the code is the strongest signal the next agent
reads. Three pairs are live today: `action_watcher` beside `holon_rule_watcher`,
`CookFormatAdapter` beside `holon-rows`, `SqlOperationProvider` as a
production-shaped writer beside Loro. Each pair is a place where the next agent
copies the wrong one.

**I4. Collapse the vision corpus to one document plus the vault.** First
principle: a vision that disagrees with itself cannot be graded. `Vision.md`,
`LongTerm.md`, `Goals.md`, `MVPs.md` and `FIRST_RELEASE_FEATURES.md` name
different primary UIs, different phase numbers, and different first-release
scopes. Keep `Vision.md` as the promise list (P1–P8), move roadmap and status
to the vault where they already live, and delete `MVPs.md` and `Goals.md`.

**I5. Pin the reference connector with the mock before touching UTCP.** First
principle: a connector nobody exercises is a promise, not a capability.
`crates/holon-mcp-mock` exists. A keystone transition that syncs, writes back
and undoes through the Todoist sidecar against the mock makes ADR 0034's
acceptance workload real, and it makes the WriteTier enforcement lane testable.

**I6. Bind `@[[Person]]:` to a durable Person today.** First principle: the
Zeroth Principle says identity precedes edges, and the cheapest edge is one the
parser already produces. A `person.yaml` type exists (`assets/default/types`).
Minting a Person entity on first delegation, without any cross-system
resolution, gives the waiting-for list of `PetriNet.md` §Delegation for free.

**I7. State explicit platform tiers in the vision.** First principle: an
unkept promise costs trust, and Holon sells trust. Say GPUI macOS is Tier 1,
Android and iOS are Tier 1 dogfood, TUI and MCP always ship, web and Windows
are parked. Delete "all browsers" and "Windows" until the storage layer can
open a database there.

**I8. Treat FeatureMap's "Unpinned" list as the roadmap's risk register.** First
principle: the keystone is the one oracle; a feature outside it is only as safe
as an argument. `op_button`, PN action language, task ranking, Todoist, pin to
sidebar and tombstones are the load-bearing rows there. Each one is a
red-first PBT away from being safe to refactor.

**I9. Write invariant 13 and the identity lint now.** First principle from ADR
0029 itself: order did not drift because it had a lint; identity drifted because
it had none. The witness types landed; the lint is a copy of
`order_minting.toml` with a new pattern.

**I10. Decide the second-writer question structurally, not per hole.** The
readonly-edits lane closes one hole in the cell-mode text path. The class is
"a write path that does not pass the dispatcher". The structural fix is to
make cell writes go through the same `WriteTierAuthority` as dispatched ops,
which the lane already sketches at `block_cell_registry.rs:109`. Land it as
the rule, not the patch.

---

## 6. Open questions for Martin

Decision-inbox style. Each has a stable id, background with anchors, a
first-principles line, options with real pros and cons, and a starred
recommendation.

### V1. Which UI premise does the vision commit to?

**Background.** Three vision documents name three primary frontends (§4 item 1).
MCP Apps hosting (`Integrations.md:7-127`) is written against a browser host
that is parked. The vault is unambiguous: GPUI on macOS, Android and iOS, TUI
and MCP as always-shipping surfaces (`Multi-Frontend Strategy.org:27-88`).

**First principles.** A vision must be gradeable. The frontend premise decides
what "custom visualizations" and "embedded third-party UI" can mean.

**Options.**

- (a) GPUI-native everywhere; visualizations are render-DSL widgets; MCP Apps
  hosting is dropped from the vision. Pro: matches the code and the vault; one
  rendering stack. Con: every third-party UI must be rebuilt as a shadow
  builder; no reuse of the MCP Apps ecosystem.
- (b) GPUI primary plus a webview widget for MCP Apps. Pro: keeps the
  ecosystem door open; the render DSL gains one `mcp_app()` builder. Con: a
  webview on GPUI and Android is a new platform dependency with its own
  sandbox story; nobody has measured it.
- (c) Revive Dioxus web as the second Tier 1 frontend. Pro: restores the
  original MCP Apps premise and the PWA story. Con: the vault parked it for
  cause; it doubles the frontend maintenance the H11 survival data says is
  already the cost center.

★ **Recommendation: (a) now, with (b) recorded as the reopening condition.**
Rewrite `Vision.md` §2 and §8 and `Integrations.md` §MCP Apps to say so. The
reopening trigger is a concrete connector whose value depends on its own UI.

### V2. What comes first: finish sync and sharing, or ship the first Watcher?

**Background.** G1 acceptance criteria are sync and sharing heavy
(`MVP Definition.org` AC-2, AC-4). The Watcher is G6. Yet every Watcher
primitive has landed (§3 G3), and the vision says P1 is the point.

**First principles.** Trust is earned by the system noticing something the
user forgot. A vault that syncs perfectly and notices nothing is LogSeq with
extra steps.

**Options.**

- (a) Keep the gate order: AC-2 and AC-4 green, then Watcher at G6. Pro: no
  new scope; pairing and sharing are half-landed and half-landed sync is a
  data-loss risk. Con: the differentiator stays invisible for months; the
  rule substrate goes untested by a real workload.
- (b) Interleave: one minimal Watcher rule pack (freshness, deadline, silent
  delegation) plus one Orient page, authored as vault data, in parallel with
  the sync lanes. Pro: zero Rust if P7 holds; validates ADR 0024 end to end;
  gives dogfooding its first "nothing forgotten" moment. Con: needs display
  placement (ADR 0015) or a canonical emission target; competes for the one
  cold build slot.
- (c) Watcher first, sync paused. Pro: fastest path to P1. Con: leaves pairing
  in the state ADR 0033 §6 calls a duplicate-id hazard.

★ **Recommendation: (b).** The rule pack should emit into a canonical place
(a "Watcher" page under journals) to avoid waiting on ADR 0015. The moment a
rule needs Rust, that is a finding, not a blocker.

### V3. How does Todoist re-enter, and what is the ADR 0034 acceptance workload?

**Background.** The vision's phases 2 and 3 are Todoist and then JIRA and
Linear. Today Todoist is a bundled MCP sidecar nobody exercises
(`FeatureMap.md` §Unpinned) and the connector program's workload is the
shopping list (ADR 0034). The kitchen crate stays until Inc 2a and Inc 4 land.

**First principles.** A connector proves its architecture only when a keystone
transition drives it through sync, write-back and undo.

**Options.**

- (a) Shopping stays the acceptance workload; Todoist follows once UTCP and
  jaq land. Pro: shopping is small and id-less, the hard reconciliation case.
  Con: id-less lists are not the shape of any system in the vision; Todoist
  has server ids and is the flagship.
- (b) Make Todoist the acceptance workload now, via `holon-mcp-mock`, before
  UTCP. Pro: pins the flagship; the mock exists; MCP transport already works.
  Con: the mock is not the wire; a second differential test against the real
  server is still owed.
- (c) Both, with the keyed reconciler (ADR 0034 §Consequences, key derivation
  as a jaq expression) built against Todoist first. Pro: forces the keyed
  case the ADR admits is unsolved. Con: widens Inc 4.

★ **Recommendation: (c), sequenced as (b) then the keyed reconciler.**
Delete `crates/holon-kitchen` in the same landing that makes the plugin path
the only path.

### V4. What is the search substrate, and does the vision stop naming Tantivy?

**Background.** FTS lives in the Turso fork and is compiled out on wasm
(`Cargo.toml:215-222`). Quick-open search is `OPEN` in the bug funnel on
`main`. The vision names Tantivy and local embeddings (`Vision.md:219`,
`AI.md` §Model Selection).

**First principles.** Search is the Integrator's first tool; the substrate
must run where the app runs.

**Options.**

- (a) Turso FTS only; embeddings deferred; web arm degrades visibly. Pro:
  already built; IVM-adjacent. Con: no semantic similarity; wasm has nothing.
- (b) Turso FTS plus an embedding index as a wasm guest connector (ADR 0034
  shape). Pro: keeps Rust minimal; platform-complete by construction. Con:
  embedding models on a phone are a size and latency budget nobody measured.
- (c) Reinstate Tantivy as an engine-side index. Pro: mature. Con: a second
  index outside IVM, the exact shape the 2026-07-11 ruling rejected.

★ **Recommendation: (a) for G1 to G3, (b) as the G4 experiment, and delete
Tantivy from the vision text.**

### V5. When does the Person entity exist?

**Background.** The Zeroth Principle (`Vision.md` §1.0) is deferred to G2
(`Entity Identity.org:50`). The parser already extracts `@[[Person]]:`
(`parser.rs:44`) and a `person.yaml` type ships in `assets/default/types`.

**First principles.** Edges need endpoints. A delegation with no Person is a
string.

**Options.**

- (a) Defer to G2 as planned. Pro: no schema churn before a second source.
  Con: the waiting-for list, the most valuable pattern in `PetriNet.md`, stays
  unbuildable.
- (b) Mint Person entities from delegation syntax now; no cross-system
  resolution. Pro: waiting-for tracking becomes a query; the type exists.
  Con: identity minting for a new derivation class must go through the ADR
  0029 authority, which still lacks its lint.

★ **Recommendation: (b), landed together with the identity lint (I9).**

### V6. Three modes: perspective data first, or the desk?

**Background.** `Principles.md` §Three Modes says the modes ship as default
layouts. Nothing exists (A33). ADR 0026 designs a desk with zones and zoom
levels; nothing exists (A32). Perspectives as data landed (A23).

**First principles.** Mode is a projection of attention, not a feature. The
cheapest projection is a perspective spec.

**Options.**

- (a) Orient and Flow as two perspective specs plus a rule pack; desk later.
  Pro: data only; reuses A23; ships with V2. Con: no spatial memory, no focus
  contract enforcement.
- (b) Build the desk data model (ADR 0026 slices 3 and 4) as the Orient mode.
  Pro: the differentiated PM surface. Con: a new document type, zone roles,
  placement blocks, and a panel; weeks of work before the first rule fires.
- (c) Both in parallel. Con: two teams on one cold build.

★ **Recommendation: (a).** Revisit (b) after the first month of dogfooding
the Orient perspective.

### V7. Do generated-doc and lint checks join the landing gate?

**Background.** CrateMap and Schema.md drifted while claiming validation
(A13, A14). ADR 0029's lint and invariant 13 are unwritten (A16). The
`analyze-arch` recipe was found to always exit 0 (ADR 0029 §Precondition).

**First principles.** A check that cannot fail is documentation, not
enforcement.

**Options.**

- (a) Add `just arch-validate`, `featuremap.py check`, and archlint with
  `pipefail` to the per-land gate. Pro: drift becomes a red. Con: adds a
  minute per landing and occasional false positives.
- (b) Run them nightly and file drift as bug-funnel entries. Pro: no landing
  friction. Con: the drift already happened under a nightly-like regime.

★ **Recommendation: (a).**

### V8. Which platforms does the vision keep?

**Background.** Vision promises web, Windows, macOS, Linux, iOS, Android.
Windows cannot open a database (`Storage.md`). Web is parked. Android and iOS
are unpinned.

**First principles.** An unkept promise in a trust product is a defect.

**Options.**

- (a) Tier the platforms in the vision as in I7. Pro: honest; frees effort.
  Con: narrows the market story.
- (b) Keep the list and add a Windows IO adapter to the Turso fork. Pro: the
  fork is Holon's; the adapter is bounded. Con: nobody dogfoods Windows.

★ **Recommendation: (a).**

### V9. When does deliberation start?

**Background.** The vault calls the deliberative layer the product
differentiator (`Engine Foundations.org:702-729`). `holon-engine` is a YAML
simulator with no loader from the native catalog (A26). The derived projection
exists (A3).

**First principles.** Simulation over a net that is not the live net proves
nothing about the user's plan.

**Options.**

- (a) Wire the projection into `holon-engine` as the first increment; what-if
  over the live vault; no learned heuristic. Pro: small; makes the projection
  earn its keep. Con: deliberation has no UI surface yet.
- (b) Defer until the Watcher and Person exist. Pro: focus. Con: the projection
  crate rots unused.

★ **Recommendation: (a), scoped to one MCP tool: `simulate_next_week`.**

### V10. Which is the first "AI" demo: Watcher, or agent supervision?

**Background.** The landscape review says the demo no competitor can do is
provenance plus re-executable blocks plus live supervision
(`UnifiedHumanAgentManagement.md` §What would move the needle most). The
primitives exist (A19, A20, `execute_source_block`). The vault's dogfooding
ideas (`Dogfooding & Agents.org`) are the same demo.

**First principles.** Dogfood what you are already doing every day; Martin
runs agents daily.

**Options.**

- (a) Watcher first (V2). Pro: P1 is the vision core. Con: needs a rule pack
  and an Orient page.
- (b) Supervision first: an Automations page query over `automations_journal`,
  an "agents needing me" view, an open-questions inbox. Pro: mostly queries
  over landed tables; immediate daily value; the differentiated demo. Con:
  narrower audience than the Watcher.

★ **Recommendation: do (b) as the first two rules of (a).** The supervision
view is a Watcher rule whose subject is agents.

### V11. Does SqlOnly stay a production mode?

**Background.** CRDT defaults ON (D69.a). The write-path unification ruling is
still open (`Architecture Alternatives.org:14-31`). Four asymmetry bugs traced
to block ops implemented twice. The readonly lane is closing one more hole in
the cell-mode path (A5, A6).

**First principles.** One semantics, pluggable evaluators (ADR 0024 P1b) also
applies to writers: two writer implementations are two semantics.

**Options.**

- (a) Keep SqlOnly as a first-class production point in the mode grid. Pro:
  degraded mode for tests and no-Loro builds. Con: every op is written twice
  forever.
- (b) Demote SqlOnly to a test and recovery mode; production writes go through
  Loro only; the SQL writer becomes the projection sink plus a test double.
  Pro: closes the second-writer class structurally. Con: the keystone loses one
  arm; the epoch guard becomes one-way.
- (c) Option A of the unification doc: one canonical op catalog over a narrow
  consolidator-strategy trait. Pro: keeps both modes with one semantics. Con:
  the largest refactor on the list.

★ **Recommendation: (c) as the target, (b) as the interim for production
builds.** Record it as the ruling the alternatives doc has waited on since
2026-07-17.

### V12. Do `Goals.md` and `MVPs.md` survive?

**Background.** Both describe systems that no longer exist (§4 items 7 and 14).

**Options.**

- (a) Delete both; `Vision.md` plus the vault carry the promise list and the
  roadmap. Pro: one source. Con: loses the historical rationale.
- (b) Move both to `docs/Archive` with a header. Pro: keeps history findable.
  Con: agents still open them.

★ **Recommendation: (b), and add a status header to every file under
`docs/Vision` and `docs/Strategy` naming the primary frontend and the date
last reconciled with the code.**

---

## 7. Proposed review lanes

Each lane is doc or read-only code work. None needs a cold build.

**L1. Vision corpus reconciliation.** Scope: `docs/Vision.md`,
`docs/Vision/*.md`, `docs/Strategy/Goals.md`, `MVPs.md`,
`FIRST_RELEASE_FEATURES.md`. Apply V1, V8, V12. Replace Tantivy and
sentence-transformers with the ruled substrate. Done when: one primary
frontend is named everywhere, platform tiers are explicit, every file carries
a "reconciled with code on <date>" header, and `MVPs.md` and `Goals.md` are
archived.

**L2. Architecture doc staleness sweep.** Scope: §4 items 2, 3, 5, 6, 9, 15,
17 in `Model.md`, `Sync.md`, `Schema.md`, `Operations.md`, `Integrations.md`,
`Replication.md`. Done when: each cited line is corrected with the code
anchor from §2, and `Schema.md`'s module table lists all seventeen modules
from `crates/holon-turso/src/schema_modules.rs`.

**L3. Generated-doc gate.** Scope: `justfile`, `scripts/featuremap.py`,
`archidoc` invocation, `archlint` recipe. Done when: `just arch-validate` and
`featuremap.py check` run in the per-land gate and fail on the current tree
(CrateMap missing five crates), and the `analyze-arch` recipe exits non-zero
on a red archlint run.

**L4. ADR 0029 closure.** Scope: `archlint/smells/identity_minting.toml`
modelled on `order_minting.toml`; `Model.md` invariant 13; the exclusion list
enumerating today's mint sites. Done when: the lint runs, the exclusion list
names every current site with a reason, and invariant 13 reads parallel to
invariant 2.

**L5. Todoist connector pin design.** Scope: `assets/integrations/todoist.yaml`,
`crates/holon-mcp-mock`, `crates/holon-integration-tests/src/pbt/transitions/`.
Write the red-first keystone transition spec (sync, write-back, undo through
the mock) and correct the FeatureMap row (MCP over HTTP, static token). Done
when: a transition spec exists with its expected red, and the FeatureMap row
matches the sidecar.

**L6. Watcher rule pack authoring.** Scope: vault data only. Three
`holon_rule` blocks (review-cadence overdue over `block_contributes_to` and
`review-cadence`; deadline within buffer over `clock`; delegation silent for
N days once V5 lands) plus one Orient page query. Done when: the rules load
without engine changes, or each engine change needed is filed as a gap with
the rule that exposed it.

**L7. Vault tracker reconciliation.** Scope: `Petri Net Execution.org:35`,
`Cross-Cutting Concerns.org:138`, `Engine Foundations.org` DOING and TODO
items, `Now.org` snapshot. Done when: every item that the code shows landed
(`holon-net`, degraded bus wiring, edge fields) is marked DONE with its
commit, following the `holon-handoff` structuring rules.

**L8. Old-path retirement inventory.** Scope: `action_watcher.rs` versus
`holon_rule_watcher.rs`; `LegacyAction`; `crates/holon-kitchen` versus
`holon-rows` and the plugin lane; `MarkdownFormatAdapter` wiring status.
Done when: each pair has a named owner lane and a deletion commit or an ADR
line saying why both stay.

**L9. FeatureMap unpinned-row triage.** Scope: `docs/Architecture/FeatureMap.md`
§Unpinned. For each row, decide: pin (name the transition and invariant),
accept as unpinned with a reason, or delete the feature. Done when: no row
lacks one of those three verdicts.

**L10. WriteTier enforcement review.** Scope: read-only review of lane
`readonly-edits` (`operation_dispatcher.rs:65-179`,
`block_cell_registry.rs:65-114`, `:286`) against A5 and I10. Done when: the
review names every write path that bypasses `WriteTierAuthority`, with the
file and line, and states whether the lane closes it.
