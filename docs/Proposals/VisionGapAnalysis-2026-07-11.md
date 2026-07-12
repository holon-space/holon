# Vision Gap Analysis — Missing Generic Primitives (2026-07-11)

Produced by a frontier-model analysis pass over ALL docs/Vision* documents (3,773 lines read in
full), docs/Architecture/Model.md, and ADR 0024, with every "exists today" claim verified against
the integration tree. Question answered (Martin's framing): the vision's workflows and AI
personalities must be definable AT RUNTIME AS DATA (blocks, profiles, queries, render DSL) — what
fundamental generic functionality is still missing?

## 1. What the vision actually demands from the substrate

The vision never asks for features — it asks for a self-describing reactive world model:
everything (tasks, people, external systems, AI personalities, scoring formulas, workflows, even
UI modes) is data in one block graph; a rules engine (CPN per ADR 0024) reacts to state changes
and emits new state; derived intelligence (WSJF, wastes, Watcher alerts, context bundles) is
queries over that graph, never code. The three AI personalities are not features: Watcher = a
bundle of rules emitting into display placement; Integrator = candidate-generating functions + a
confirmation queue; Guide = queries over HISTORY. That last word is the tell: roughly half the
vision (Guide, Shadow Work, trust ladder, agent supervision, "postponed 7 times", velocity) is
defined over change-over-time, while Layer 4 is explicitly convergent state, not an event log.
The other recurring demand is the outside world as replicas: Digital Twins, Todoist/JIRA/Gmail,
browser signals — Model.md Layer 1 reserves the slot, zero connectors exist. Almost everything
else is already expressible as data today or lands with the remaining ADR 0024 phases.

## 2. Capability -> status map

A = expressible today as data - B = pending decided track - C = missing primitive (section 3)

| Vision capability | Status | Mechanism / pending track / missing primitive |
|---|---|---|
| Rules "query -> action" (Vision s3, PRQL automation) | B | ADR 0024 Phase 3 (holon_rule, one output model) |
| Tasks-as-transitions, delegation sub-nets, waiting_for, inhibitor at-most-once | B | ADR 0024 Phase 4 + C7 (syntax parser feeds attributes) |
| Simulation / what-if / "delegate this project?" | B | ADR 0024 Phase 5 (deliberation); engine CLI already forks markings |
| Objective function & WSJF as prototype blocks with = Rhai props | A (narrow) | holon-petri PrototypeValue/DEFAULT_TASK_PROTOTYPE/rank_tasks — materialize-on-demand only -> C4 for reactive |
| Kanban / lifecycle projections / SOP views | B + A | Position-marking boards = Phase 4; pipeline views = queries + profiles today |
| Question/Information tokens, confidence, via: routes | A | Block types + properties + edge fields + profiles; routing rules = Phase 3 |
| Checklists, > dependency, priorities/deadlines | A / C7 | Data model fits; the parse of @ ? > verb-grammar is missing |
| Clock/temporal guards, daily journal | A (landed) | clock relation + scheduler + advance_day; day-grain only -> C6 for recurrence/hours/decay |
| Watcher: alerts, blocked-transition detection, deadline risk | B + C2 | Rules over state (Phase 3) OK; divergence/velocity/staleness need history relation |
| Guide: Shadow Work, postponement counts, patterns over time | C2 | Fundamentally history-shaped |
| Integrator: confirmation-driven edge proposals, entity resolution | B + C3/C5 | Emission-into-display + suppression anti-join right shape; candidate generation + confirm-promotes missing |
| Unified / semantic search | C3 | ABSENT: no Tantivy, no embeddings, no FTS in workspace |
| AI Trust Ladder, autonomy per transition | C5 (landed 2026-07-12) | Trust gate at the dispatch boundary: coerced proposal emission + accept/reject promotion + `trust_proposals` supervision matview |
| Third-party systems first-class, Digital Twins, bi-dir sync | C1 | Zero integration code; Layer-1 slot reserved; mcp-yaml-sidecars directive points the way |
| Agent provenance per block, revert-whole-call, supervision view | C2 mostly | OpOrigin exists but does not reach block properties; then supervision = one query |
| Re-executable source blocks | A partial | execute_source_block MCP tool exists; provenance stamping of outputs = C2; MCP-proxy = C1 |
| Self DT dynamics (energy/focus/flow_depth), emergent Pomodoro | C4+C6+C1 | Computed decay fields + fine clock + signal connectors |
| Three UI modes as adaptable perspectives | C8 partial | Perspective-as-data primitive landed (`holon_api::perspective`: typed `PerspectiveSpec`, active-perspective pointer, resolver); reactive render consumption across the two render-derivation arms deferred — see `docs/Proposals/PerspectivesAsData-C8.md` |
| Sharing subtrees, team features | park | Loro sync exists; scoped permissions = Phase-7 vision, no track |
| Behaviour-tree policy layer | rejected/parked | PetriNet.md explicitly defers |

## 3. Ranked missing fundamental primitives

C2 — History & provenance as a queryable relation (biggest unlock/effort ratio; ADR 0024 P8 path).
Stamp every engine/MCP op's outputs with provenance properties (tool-call-id, agent-id, rule-id,
timestamp — OpOrigin exists but doesn't reach block properties); project the op/effect stream into
Turso as a DISCLOSED EPHEMERAL CACHE relation (rebuildable from Loro history; fidelity ladder
Loro > jj > none; never authoritative — Layer-4-conformant). Downstream all becomes user data:
Guide rules, Automations page, supervision view, per-rule acceptance stats feeding trust ladder.
Risk low-medium.

C3 — Extensible set-valued/scoring function registry (search + similarity as guard/query builtins).
advice_candidates() is the ratified precedent. Registry of engine-provided functions usable in
Pattern guards, output arcs, queries: fulltext(query), similar(block,k), attr_match(email),
content_matches(pattern). Engine-level because IVM can't maintain a Tantivy/embedding index; engine
maintains index from CDC, exposes as relation/TVF. Unlocks unified search, Integrator proposals,
entity resolution, context bundles. Risk medium.

C1 — Declarative external-replica connectors (Digital Twins as data). Connector engine consuming a
data-level twin definition (resource->block-shape mapping, op->MCP-tool mapping, sync policy) and
instantiating a Layer-1 replica with own base, diffed intent, lease-governed external effects (ADR
0024 P4 taxonomy, ratified). Matches mcp-yaml-sidecars directive + existing mcp-client crate.
Largest vision surface; risk high but architecture slot pre-reserved.

C4 — Maintained derived fields (computed = properties in the reactive pipeline). SEAT LANDED
2026-07-12: hybrid seat behind the `Computation` interface. `DerivedFieldPlan::plan`
(holon-api/src/computation.rs) routes each declared field by `compile_sql()` — Ok → planted as an
IVM-maintained matview column (`block_matview_select_with_computed`, proven O(delta)
maintain/replace/retract against real Turso in holon-turso/tests/derived_field_matview.rs); Err →
DISCLOSED projection-stage evaluation via `Computation::eval` (`evaluate_stage`, fail-loud,
retraction-by-overwrite). Field-value analogue of ADR 0024 maintained display emission; the
production seat-B home is the enrich boundary (ui_watcher `resolve_computed_fields`). REMAINING
(non-blocking): (1) trigger wire feeding prototype-block-declared fields into `plan()` at reconcile
time; (2) routing the production enrich path through the fail-loud `Computation` evaluator;
(3) `rank_tasks` convergence. See docs/Proposals/ComputationTrait-2026-07-11.md.

C5 — Autonomy/trust enforcement at the intent boundary. Representation trivial; enforcement
engine-level. Elegant form: below-threshold origins may only emit into display/proposal places
(maintained, retractable); confirmation = ordinary intent re-emitting into canonical place. Derives
the safety property instead of asserting it. Risk low; shape needs ruling.
LANDED 2026-07-12 (coerced-emission form, pending ratification): `TrustPolicy` in holon-profiles
(typed origin-class/entity/operation rules, first match wins, no match = trusted; YAML
parse-don't-validate); gate at `DispatchingOperationEngine::execute_operation` coerces
sub-threshold dispatches into proposal blocks under `block:proposals` (deterministic proposal id
per ADR 0024 P4 — re-fires converge; `_proposal` carries the wrapped op verbatim; `_provenance`
names the proposer); `accept_proposal` re-dispatches the wrapped op with the confirmer's origin
(dual provenance: `_provenance` = confirmer, `_proposed_by` = proposer), `reject_proposal`
retracts to a terminal status; supervision = `trust_proposals` matview (IVM over block_raw) +
`TRUST_PROPOSAL_STATS_SQL` aggregate. Deferred: loading a policy from vault profile blocks (the
default is trust-all, so the gate is a no-op until configured); UI surfaces.

C6 — Clock generalization: hour/minute grains + recurrence builtins (every(...) desugars to
read-arcs on clock relation). Pure extension of landed Phase-1 code. Risk low (watch fine-grain
write amplification).

C7 — Task-syntax boundary parser with dictionaries-as-blocks. Deterministic @ / ? / > /
verb-object / bare-noun parser (fully specified in PetriNet.md incl. safe defaults); verb dictionary
and via:-routing rules are blocks, user-extensible at runtime. Risk low.

C8 — Perspectives/layouts as data (minor). Named mode = block declaring panel queries, profile
overrides, concealment parameters. Rank last. PARTIALLY LANDED: the data primitive exists
(`holon_api::perspective` — typed `PerspectiveSpec`/`PanelSpec`/`ConcealmentParams` parsed
parse-don't-validate at the boundary, an `active_perspective` pointer property on the root-layout
block that persists like collapse state, and `resolve_active_perspective`). Deferred: wiring the
`activate_perspective` op into the dispatcher and teaching the two reactive render-derivation arms
(`BlockDomain::render_entity` Turso / `loro_ui_watcher::derive_render_expr` no-Turso) to resolve
panels through that pointer so the live layout swaps without restart. Full design + exact seam:
`docs/Proposals/PerspectivesAsData-C8.md`.

NOT proposed (already decided/rejected): behaviour trees (parked), execution-log-as-dedup
(rejected ADR 0024), position-marking as default (rejected), second rule language (rejected), any
authoritative store beside the block tree.

## 4. Suggested build order

Increment 1 — no ruling needed (inside ratified scope / fully specified):
- C6 clock grains + recurrence.
- C2a provenance stamping: OpOrigin -> block properties for rule- and MCP-agent-authored blocks
  (Phase 3 mandates fired-by; extend to agent/tool-call ids). Alone enables the supervision query.
- C7 syntax parser + dictionary blocks.
- Continue ADR 0024 Phases 2-3 as planned (prerequisite for authoring personalities as rules).

Increment 2 — one ruling each, then fleet-executable:
- C2b history projection. RULED (Martin 2026-07-11): Turso cache APPROVED; abstract behind an
  interface if at all possible (unless directly exposed as SQL query). Org-standalone operates in
  DEGRADED MODE with reduced functionality (precedent: CRDT vs LWW).
- C4 derived fields. RULED (Martin 2026-07-11, direction): hide behind an interface; candidate
  design = generalize the existing Predicate trait to a Computation trait evaluable in memory AND
  compilable to SQL. Pipeline seat still open behind that interface. SEAT LANDED 2026-07-12 as a
  HYBRID: `DerivedFieldPlan` routes by `compile_sql()` → IVM matview column (seat A) or disclosed
  projection-stage `eval` (seat B). Both halves proven; remaining = trigger/enrich wiring +
  rank_tasks convergence (see ComputationTrait doc).
- C3 function registry. RULED (Martin 2026-07-11): TURSO for FTS (in-fork extension). Embeddings
  question deferred (fulltext-first). SCOUT UPDATE: fork ALREADY has Tantivy-backed FTS
  (core/index_method/fts.rs, fts_match/fts_score/fts_highlight in Func resolution, feature `fts`,
  non-wasm) + a sparse-vector index method (future similar()). Remaining scope: enable feature
  through holon dep graph (off for wasm32), verify index-maintenance contract, registry seam.
  REGISTRY RULED (Martin): generalize the fork's Func-enum resolution path; by shape —
  scalar/predicate funcs via Func enum, set-valued funcs via matview/TVF declaration path
  (note: advice is matview+IVM anti-join, NOT a UDF — earlier premise corrected).

Increment 3:
- C1 connectors. RULED (Martin 2026-07-11): Todoist-class connectors are ALREADY EXPRESSIBLE today
  via MCP + yaml sidecar (working examples: docs/integrations/todoist.yaml, claude-history.yaml,
  assets/queries/todoist_hierarchy.prql) — C1 demotes to generalization: document the recipe, then
  add a second transport where the sidecar describes direct HTTP-API interaction instead of a
  server — UTCP manuals / OpenAPI-derived (UTCP can also describe MCP, so transports stay plural:
  mcp | http(UTCP/OpenAPI) | graphql-later). Leases/read-write question unchanged.
  LANDED (2026-07-12): (1) connector recipe documented — docs/integrations/README.md (resource→
  block-shape, op→tool, sync policy, transports; Todoist walked through). (2) Transport plurality:
  `transport: rest` added to the sidecar schema (serde deny_unknown_fields, typed, `${VAR}`-only
  secrets) alongside the existing `child_process`/`http`(=MCP-over-HTTP) MCP transports; a
  `RestCallSurface` serves a UTCP-style manual (base_url + GET `calls` + `{arg}`/`result_key`
  mapping) behind the SAME `McpCallSurface`/`SyncStrategy` read seam — one engine, plural
  transports (holon-mcp-client/src/rest_transport.rs). (3) Read-only example jsonplaceholder.yaml +
  end-to-end test against a LOCAL mock server (holon-mcp-client/tests/rest_transport_mock.rs, no
  network). STILL OPEN (unchanged): leases/read-write; and `rest` background-runner wiring (prod
  runner is built on MCP resource subscriptions, which a plain HTTP API can't serve — poll-only
  runner is the remaining step; build_mcp_integration fails loud for a `rest` sidecar until then).
- C5 trust gate. RULING: literally "sub-threshold origins coerced to display-place emission", or a
  separate permission check at the dispatcher? (Recommended: the former.)
  BUILT 2026-07-12 as the recommended form — coerced emission, not a dispatcher permission check
  (see §3 C5 LANDED note). Awaiting Martin's ratification of the ruling wording.

Through-line: after Increment 2, all three AI personalities become authorable as vault data —
Watcher = rules + clock + display emissions; Guide = rules over the history relation; Integrator =
rules over registry functions + proposal/confirm places — zero personality-specific engine code,
which is exactly the substitution test the vision demands.
