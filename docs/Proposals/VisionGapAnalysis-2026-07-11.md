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
| AI Trust Ladder, autonomy per transition | C5 | No trust metadata; ADR 0024 place kinds are the natural enforcement lever |
| Third-party systems first-class, Digital Twins, bi-dir sync | C1 | Zero integration code; Layer-1 slot reserved; mcp-yaml-sidecars directive points the way |
| Agent provenance per block, revert-whole-call, supervision view | C2 mostly | OpOrigin exists but does not reach block properties; then supervision = one query |
| Re-executable source blocks | A partial | execute_source_block MCP tool exists; provenance stamping of outputs = C2; MCP-proxy = C1 |
| Self DT dynamics (energy/focus/flow_depth), emergent Pomodoro | C4+C6+C1 | Computed decay fields + fine clock + signal connectors |
| Three UI modes as adaptable perspectives | C8 minor | Profiles/queries cover content; named layout/perspective blocks don't exist |
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

C4 — Maintained derived fields (computed = properties in the reactive pipeline). Prototype-block
machinery exists only in holon-petri at materialize time (rank_tasks). Needed: view semantics
(recompute/retract on input change) so any matview/profile selects computed fields live. Field-value
analogue of ADR 0024 maintained display emission. Risk medium (IVM interaction).

C5 — Autonomy/trust enforcement at the intent boundary. Representation trivial; enforcement
engine-level. Elegant form: below-threshold origins may only emit into display/proposal places
(maintained, retractable); confirmation = ordinary intent re-emitting into canonical place. Derives
the safety property instead of asserting it. Risk low; shape needs ruling.

C6 — Clock generalization: hour/minute grains + recurrence builtins (every(...) desugars to
read-arcs on clock relation). Pure extension of landed Phase-1 code. Risk low (watch fine-grain
write amplification).

C7 — Task-syntax boundary parser with dictionaries-as-blocks. Deterministic @ / ? / > /
verb-object / bare-noun parser (fully specified in PetriNet.md incl. safe defaults); verb dictionary
and via:-routing rules are blocks, user-extensible at runtime. Risk low.

C8 — Perspectives/layouts as data (minor). Named mode = block declaring panel queries, profile
overrides, concealment parameters. Rank last.

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
- C2b history projection. RULING: Turso-projected op/effect relation acceptable as disclosed
  ephemeral cache? Degraded form in org-only vaults: jj-commit granularity or none?
- C4 derived fields. RULING: pipeline seat — compile = props to matview expressions, system rule
  emitting attribute updates, or CDC-incremental materializer?
- C3 function registry. RULING: index substrate — Tantivy sidecar from CDC vs in-Turso FTS
  extension (we own the fork); embeddings in v1 or fulltext-first?

Increment 3:
- C1 connectors. RULINGS: (a) YAML-sidecar twin-definition schema as authoring surface; (b) first
  target (Todoist per Vision Phase 2?); (c) leases before or with first read-write connector —
  read-only twin first de-risks.
- C5 trust gate. RULING: literally "sub-threshold origins coerced to display-place emission", or a
  separate permission check at the dispatcher? (Recommended: the former.)

Through-line: after Increment 2, all three AI personalities become authorable as vault data —
Watcher = rules + clock + display emissions; Guide = rules over the history relation; Integrator =
rules over registry functions + proposal/confirm places — zero personality-specific engine code,
which is exactly the substitution test the vision demands.
