# C2 — History & Provenance as a Queryable Relation: Completion Plan (2026-07-16)

Planner: C2 planning session 45a759ad. Repo read-only; all claims carry file:line evidence
(verified 2026-07-16 against working tree @ HEAD, branch HEAD, last commit 079014efeb).

## 0. Premise correction — the docs are stale, most of C2 is LANDED

VisionGapAnalysis-2026-07-11 §3 describes C2 as future work ("OpOrigin exists but does not
reach block properties"). The tree has moved past that:

**C2a (stamping) — LANDED.**
- `ProvenanceStamp` newtype, `PROVENANCE_PROPERTY = "_provenance"`, fail-loud parse:
  `crates/holon-api/src/provenance.rs:31,48-60,104-137`.
- Stamped at the dispatch chokepoint for all authoring ops:
  `crates/holon/src/api/operation_engine.rs:343` (`stamp_provenance`), applied at `:801`.
- Rule-fired creates carry `origin=rule` + `transition_id`:
  `crates/holon/src/api/holon_rule_watcher.rs:656-679` (test proves it).
- Trust proposals carry dual provenance (`_provenance` = confirmer, `_proposed_by` = proposer):
  `operation_engine.rs:561-563,685`; matview test `crates/holon/tests/trust_proposals_matview.rs:19`.

**C2b (history relation) — WRITE SIDE LANDED.**
- Trait + event + query types: `crates/holon-api/src/history.rs` — `HistoryStore` (:131),
  `HistoryEvent` (:57, per-field-delta: block_id, op_name, origin, transition_id, session_id,
  tool_call_id, field, new_value, at_millis), `HistoryQuery` (:83, incl. `transitions_to`
  "postponed N times" shape :111), `HistoryFidelity` ladder Loro≻Jj≻None (:33).
- Turso impl: `crates/holon/src/api/history_store.rs` — `TursoHistoryStore` (:46), table
  `block_history` + 3 indexes created lazily on first use (:64-81), `DegradedHistoryStore`
  (:233, loud no-op reads for org-standalone).
- Wired in production: `crates/holon/src/api/backend_engine.rs:116,793`
  (`.with_history_store(...)`).
- Recorded unconditionally per successful op, one event per field delta, fail-loud:
  `operation_engine.rs:577-579` (create), `:877-881` (generic dispatch), builder
  `history_events_for` (:165). NOTE: the "only User ops enter history" comment at `:831`
  is about the UNDO stack, not the C2b relation — history records ALL origins that pass
  through `execute_operation`.
- Determinism/ephemerality proof exists as replay-the-same-stream test:
  `history_store.rs:364-398` (`rebuild_from_stream_reproduces_relation`).

**Ruling context already fixed (do not re-litigate):**
- ADR 0024 P8 (`docs/adr/0024-unified-action-execution.md:173-190`): Turso audit table is only
  ever a cache; journal = a query; fidelity ladder Loro ≻ jj ≻ none, disclosed.
- Martin's ruling 2026-07-11 (VisionGapAnalysis:133-135): Turso cache APPROVED; interface
  abstraction; direct SQL exposure allowed; org-standalone = disclosed degraded mode.
- ADR 0025 (`docs/adr/0025-op-grounded-projections.md:12-13,55-57`): ops are the sole
  propagation currency; the recovery-path writeback (`re_render_all_tracked`) grounding and
  mass-truncation-tripwire tightening are explicitly deferred TO the C2b history relation
  (hooks named in code: `crates/holon-filesystem/src/file_sync_controller.rs:83,2607,2751,3203`;
  `crates/holon-orgmode/src/writeback_guard.rs:52`).

Therefore this plan is a **gap-completion plan**, not a build plan.

## 1. First principles

**Goal.** One queryable relation such that these become plain queries (no personality-specific
engine code — the vision's substitution test):
- Q1 Supervision: "everything agent session S did, grouped by tool call" (`HistoryQuery::for_session`, history.rs:103).
- Q2 Guide: "block B moved to `postponed` N times", staleness/velocity over time windows.
- Q3 Automations journal: "effects grouped by rule and day" (ADR 0024 P8 wording, 0024:188-190).
- Q4 Trust stats: per-rule/per-agent proposal acceptance (already per `TRUST_PROPOSAL_STATS_SQL`,
  `crates/holon-turso/src/schema_modules.rs:426-430`) JOINED with fire counts from history.
- Q5 Forensic: "why does this block have this state" — filter history by provenance (0024:181).
- Q6 (ADR 0025 follow-up, adjacent): "is this projection-observed absence grounded in a recorded
  delete op?" — the recovery-writeback grounding query.

**Constraints.**
- Write path must not slow: SLO p95 interaction→projection-visible < 200ms (CLAUDE.md; latency
  is a bug class). History insert sits ON the dispatch critical path and fails loud
  (operation_engine.rs:875-881) — overhead must be measured, and bounded per op, not per delta.
- Disclosed ephemeral cache, never authoritative (Layer 3 by contract, 0024:175-176). Corollary
  that works FOR us: schema migration = drop + recreate; no migration machinery ever.
- CDC fires only via matviews (durable principle) → anything a RULE must react to needs a
  matview over `block_history`; direct SQL suffices for pull-queries only.
- Compose with ADR 0024: deterministic effect-IDs (`crates/holon-api/src/effect_id.rs`) and
  time-as-data clock (`crates/holon-api/src/clock.rs`) are landed Phase-1 pieces; history events
  already take `at_millis` from the injected Clock (operation_engine.rs:363).
- Block writes must use `db_handle.transaction()` (deferred-FK autocommit wart, memory).
  `block_history` has no FKs, but batching wants a transaction anyway.

**What is durable truth vs cache.** Truth = Loro op history + block `_provenance` stamps (+ jj
for org-standalone). `block_history` = cache. Fidelity is a *rebuild guarantee statement about
the substrate*, not about live contents (history.rs:28-31) — but today no rebuilder exists
(§Gap G4), so the guarantee is asserted, not demonstrated.

## 2. Verified gap inventory

- **G1 No consumers.** Nothing outside history_store.rs/operation_engine.rs queries
  `block_history` (grep `HistoryQuery|block_history` across crates+frontends: only
  holon-api/lib.rs re-export and a PBT capability-name near-miss in
  holon-integration-tests/src/pbt/frontend_slice/components.rs:51 which is `SutHistoryWrite`
  for *navigation* history, unrelated). Q1–Q5 are all still unbuilt.
- **G2 Not part of the boot schema / no matview.** Table is created lazily inside the typed
  accessor (history_store.rs:64-81), not via a `SchemaModule` like every other relation
  (schema_modules.rs pattern; matviews registered via `reconcile_named_view`,
  `crates/holon-turso/src/matview_manager.rs:59-90`, e.g. `TrustProposalsSchemaModule`
  schema_modules.rs:421-457). Consequences: table invisible to PRQL/raw-SQL/`list_tables`
  until the first op; first op pays DDL latency; and — decisive — no matview means no CDC,
  so Guide rules cannot fire on history.
- **G3 No MCP/agent exposure.** `frontends/mcp/src/tools.rs` has no history tool;
  `list_operations` (frontends/holon-worker/src/lib.rs:1086) lists registered op *descriptors*,
  not history. Agents can only reach the relation via `execute_raw_sql` — undocumented.
- **G4 No rebuild-from-Loro.** `rebuild_from_stream_reproduces_relation`
  (history_store.rs:364) replays an in-memory stream — it proves determinism, not
  rebuildability from the substrate. Additionally, per-op provenance does NOT ride Loro
  commits (scout-verified: `subscribe_root`/`extract_pending_changes` in
  crates/holon-loro/src/loro_sync_controller.rs:184-190, loro_backend.rs:1352; no commit-message
  channel used) — only the *latest* `_provenance` block property survives in Loro state, so a
  full-fidelity rebuild is not currently possible even in principle. Fidelity::Loro is
  over-claimed today.
- **G5 Origin coverage = engine-dispatched ops only.** The Loro→Turso consolidator applies
  sync-origin CRDT deltas via `command_bus.execute_batch_with_origin(..., EventOrigin::Loro)`
  (crates/holon-loro/src/consolidator.rs:125-127), bypassing `execute_operation` — peer edits
  and (depending on wiring) ingest deltas produce NO history events. Acceptable if disclosed;
  fatal for the ADR 0025 recovery-grounding use (Q6), which needs delete events from all origins.
- **G6 Event shape gaps** (cheap now — ephemeral ⇒ drop+recreate; expensive later):
  no `old_value` (forensics + inverse derivation; `FieldDelta` carries it — see
  `Precondition::forward(&result.changes)` usage operation_engine.rs:866), no per-op group id
  (an op's N field deltas are indistinguishable from N ops; tool_call_id groups agent CALLS but
  a rule firing instance has no id), no effect_id column (ADR 0024 P4 deterministic-ID effects
  should be joinable), no `entity_name` (ops on non-block entities).
- **G7 Unbounded growth, undisclosed.** Undo `operations` table trims to 100
  (crates/holon/src/core/operation_log.rs:23-30); `block_history` never trims. Fine for v1
  IF disclosed and measured at vault scale.
- **G8 Degraded store possibly unwired.** `DegradedHistoryStore` has no construction site
  outside its own file + mod.rs re-export (grep `DegradedHistoryStore` — only
  backend_engine.rs:115 *comment*, mod.rs:53). No-Turso wirings leave `history: None`
  (operation_engine.rs:251) — silently, not disclosed-degraded. Implementer must re-verify.
- **G9 No keystone/PBT oracle.** Keystone composed PBT has no history-relation invariant.

## 3. Non-goals (v1)

- Revert-whole-call EXECUTION (supervision action). We record enough to derive inverses later
  (old_value + op_group per G6) but build no revert machinery; user-origin undo stack stays the
  only reverser (operation_engine.rs:831 Ruling #1 untouched).
- Retention/compaction machinery. Disclose unbounded growth; measure; defer policy.
- jj-fidelity rung implementation (org-standalone stays `HistoryFidelity::None` + degraded reads).
- Trust-ladder UI, Automations PAGE rendering — we deliver the queries; page wiring is C8/UI work.
- Embeddings/search over history (C3 territory).
- Backfilling history for pre-C2b vault data (rebuild delivers what the substrate can prove;
  no synthetic events).
- Second write path: no direct-SQL "insert into block_history" for anyone but HistoryStore.

## 4. Increments (each independently landable, keystone-green)

Keystone gate for every increment:
`cargo nextest run -p holon-integration-tests --features pbt -E "test(general_e2e_composed_pbt)"`
(accepted baseline: pre-existing journals ingest-data-loss RED; anything else red = stop).
Feature-gate hazard: pass member features explicitly (memory: gate-required-features-blindspot —
`--features holon-integration-tests/pbt,holon-gpui/pbt` where relevant).

### INC 1 — Schema truth: boot-registered table, final event shape, batched writes
**De-risks the scariest remaining unknowns first: write-path overhead + schema finality.**
- Move `block_history` DDL into a `HistorySchemaModule` (pattern: schema_modules.rs; keep
  `ensure_schema` as an idempotent belt-and-braces or delete it — prefer delete, one owner).
- Finalize event shape while drop+recreate is free (G6): add `entity_name`, `old_value`,
  `op_group` (one id per `execute_operation` call — **A1: minted DETERMINISTICALLY**, a
  session-scoped monotonic sequence or the ADR 0024 effect-id machinery, NEVER a uuid/random
  at the chokepoint — a random id breaks `rebuild_from_stream_reproduces_relation` and PBT
  replay determinism), `effect_id NULLABLE` (populated when the op came from an ADR 0024
  effect; wiring may be a follow-up, column lands now), and **A5: a precomputed `day` column**
  (derived from `at_millis` at insert) so INC 2's journal matview does not depend on the fork
  IVM maintaining an at_millis→day expression — if the INC 2 spike proves expression
  maintenance, the column stays as a cheap denormalization; if not, it is load-bearing.
  Update `HistoryEvent`/`HistoryQuery`/`where_clause`
  (history_store.rs:85-114) symmetrically. DDL carries a schema-version comment; version bump ⇒
  drop+recreate (ephemerality made load-bearing; extend module docs history.rs:9-22).
- **A3 (deliberate convergence, recorded not built):** `op_group` + `old_value` are exactly the
  shape "undo entries as data" (undo ruling: A-shaped-for-C, ADR 0024 later) needs — one op
  group = one undo entry, old_value = the inverse payload. The undo stack stays untouched in
  C2; this schema decision deliberately keeps that door open.
- Batch: `record_history` currently awaits one INSERT per field delta
  (operation_engine.rs:356-365) in autocommit. Change `HistoryStore::record` to
  `record_batch(Vec<HistoryEvent>)` (or add it), Turso impl = one transaction, multi-row insert.
- Measure: drive the existing latency harness (`crates/holon-api/src/latency_e2e.rs`,
  holon_latency tracing per memory latency-vault-verdict) with history wired vs unwired; record
  numbers in the PR description. Acceptance: history adds < 5ms p95 per interaction on the
  desktop wiring. **A4:** do NOT run concurrently with the latency-slo investigation lane
  benchmarking on this machine — measure when the machine is quiet, or reuse that lane's
  baseline numbers (logs in /tmp/, workspace latency-slo).
- Fix G8: wire `DegradedHistoryStore` in the no-Turso session builder
  (crates/holon-app/src/no_turso.rs / headless_builder_services.rs — implementer locates the
  engine construction) so degraded mode is disclosed, per history.rs:19-22 contract.
- DONE gate: keystone green; latency numbers documented; `list_tables` (MCP) shows
  `block_history` at boot; unit tests updated for new columns; degraded wiring disclosed-loud test.
- Tier: **executor**. Blast radius: holon-api/src/history.rs, holon/src/api/{history_store,
  operation_engine,backend_engine}.rs, holon-turso schema_modules.rs, holon-app wiring. No
  frontend surface.

### INC 2 — Reactivity spike: one matview over block_history, CDC-proven
**De-risks "Guide rules over history" — the C2 promise depends on it.**
- Add `automations_journal` matview: effects grouped by (origin, transition_id, day) —
  exactly ADR 0024 P8's journal query (0024:186-190) — registered via `reconcile_named_view`
  like trust_proposals (schema_modules.rs:421-457). Matview is over a TABLE (not
  matview-on-matview), so the chained-matview hang class (skill turso-chained-matview-hang)
  should not apply — verify anyway; the fork is ours (memory turso-ivm-ours) if grouped
  aggregation over a base table needs IVM work.
- Prove CDC: integration test — `watch_query` on the matview, dispatch a rule-origin op, assert
  a `Change` arrives (the rule-firing trigger shape); O(delta) maintenance assert in the style
  of derived_field_matview tests (crates/holon-turso/tests/derived_field_matview.rs precedent).
- Keystone oracle (G9) — **LANDED as TWO cap-gated correspondences, exact-equality REFUTED.**
  The originally-planned invariant "count of model-applied mutating ops == count(block_history)
  delta per case" was refuted as false-by-design (see §5/A2). What landed instead, both gated on
  a new `SutHistory` cap (present only on the Turso frontend arm — org-only draws deselect
  cleanly) and checked at the combined-fixed-point settle (R6):
  - `inv-history-no-phantom-rows/block_history` — SUBSET check: every `block_id` recorded in
    `block_history` ⊆ (ref live universe ∪ every id the oracle minted). Catches PHANTOM history
    (a mis-keyed / leaked recording). The ever-created anchor is derived in the harness
    `run_report` from the `IdResolver` map (which retains create-then-deleted ids), so no
    id-space remapping or ref-core hooks are needed.
  - `inv-history-records-all-creates/block_history` — LOWER BOUND: `count(DISTINCT op_group) >=`
    the number of UI-driven (synthetic→real reconciled) creates the oracle drove. Catches MISSED
    history (a create that silently failed to record). Born-equal external/peer creates
    (`key == value` in the reconcile map — org-ingest / Loro degraded-store path, no engine
    history) are excluded so the bound never over-counts. Conservative `>=` (extra SUT
    recordings — edits, boot-rule firings — only help).
  - Non-vacuity proven by four doc-§6 catch/pass tests in `composed/correspondences.rs`
    (stub `SutHistory`); keystone shows the invariants select on full-headless draws and add
    ZERO red beyond the pre-existing journals baseline. Files: `holon-pbt-core` capabilities
    (`SutHistory`, `RefHistoryExpectation`), `composed/{correspondences,catalog,harness,builder}.rs`,
    `frontend_slice/components.rs`, `ref_caps/{blocks,mod}.rs`, `reference_state.rs`.
- **PARKED (needs its own increment):** the full QUANTITATIVE per-case op-group count-delta
  (exact equality, or per-kind counting of set_field/delete/split/toggle). Sound counting of
  those kinds requires a `records_history` classifier threaded through the macro-generated
  transition-dispatch enum with per-variant + `MutationSource` + ref-state-change gating (peer
  merges via the degraded store, variable-cardinality editor ops, and zero-delta ops each break
  a naive count). The create-only lower bound above is the sound subset of that idea landable
  now; extend the counted set in a follow-up once the classifier exists.
- DONE gate: keystone green incl. new oracles (no new red vs baseline); CDC test green; matview
  visible in `list_tables` (smoke test `automations_journal_matview.rs`).
- Tier: **executor** (Turso/IVM familiarity). Blast radius: holon-turso schema_modules +
  tests, holon-integration-tests keystone oracle. RISK R2 below is the watch item.

### INC 3 — Consumption: the query pack + MCP exposure (the product payoff)
- Canonical queries as data (assets/queries/ or profile blocks, matching the Todoist PRQL
  precedent assets/queries/todoist_hierarchy.prql):
  Q1 supervision-per-session/tool-call, Q2 transitions_to counts, Q3 automations journal
  (reads INC 2 matview), Q4 trust stats join (`TRUST_PROPOSAL_STATS_SQL` × history fire counts),
  Q5 forensic per-block timeline.
- MCP: one thin `query_history` tool in frontends/mcp/src/tools.rs (+ holon-worker parity,
  frontends/holon-worker/src/lib.rs:1086 region) mirroring `HistoryQuery` + `count` —
  parse-don't-validate on the filter; document that raw SQL over `block_history` is equally
  sanctioned (ruling allows direct SQL, history.rs:13-16). Also serves dogfood-explorer and
  supervision agents immediately.
- DONE gate: keystone green; MCP integration test (dispatch op → query_history sees it);
  each Q1–Q5 exercised by at least one test; queries documented in docs (repo docs hold exact
  identifiers; vault entry phrased around them per CLAUDE.md).
- Tier: Q-pack + MCP = **mech-executor** (fully specified); trust-join query design =
  **executor**. Blast radius: frontends/mcp, frontends/holon-worker, assets/queries, docs.

### INC 4 — Fidelity honesty: rebuild story (RULING REQUIRED — fork F2 below)
- **STATUS 2026-07-16 — MINIMUM (F2b honest-partial) LANDED; FULL (F2a Loro fidelity) PARKED
  awaiting Martin's ruling.** Shipped:
  - New `HistoryFidelity::Partial` variant (`crates/holon-api/src/history.rs`) — recovers the
    block-stamp create-provenance subset, never the full op stream.
  - Fidelity is now COMPUTED, not caller-asserted: `TursoHistoryStore::new` dropped its
    `fidelity` parameter; `fidelity()` returns `Partial` (the guarantee `rebuild` actually
    delivers). Removed the rejected `HistoryFidelity::Loro` over-claim at every call site
    (`backend_engine.rs`, `holon_service.rs`, tests).
  - `HistoryStore::rebuild()` implemented (`crates/holon/src/api/history_store.rs`): truncates
    `block_history`, then replays one `create` event per extant block carrying its `_provenance`
    stamp (read from `block_raw` via `json_extract`, ordered `(at_millis, id)` for determinism).
    Field-delta history left no substrate trace → omitted (not fabricated). `DegradedHistoryStore`
    rebuild fails loud (no substrate). Rebuild contract disclosed in the `history.rs` module docs.
  - Tests: `rebuild_recovers_create_provenance_subset_and_is_deterministic` (create-provenance
    subset recovered, field-deltas provably NOT, two rebuilds byte-identical, `fidelity() ==
    Partial`); degraded-rebuild-fails-loud assertion added. All history lib + integration tests
    green; keystone signature = accepted journals ingest-data-loss baseline only (no regression).
- Minimum (no ruling needed): implement `rebuild()` = drop table + replay what the substrate
  CAN prove — a disclosed partial rebuild (creates with their stamped `_provenance`; everything
  else with `origin=unknown` or omitted) — and change the constructor-asserted
  `HistoryFidelity::Loro` to a computed, honest value until full fidelity exists. Extends the
  history_store.rs:364 test from replay-determinism to substrate-rebuild.
- Full (needs ruling): ride provenance on Loro commit metadata/messages at the write seam so
  the op stream is losslessly recoverable → true Fidelity::Loro. Touches consolidator commit
  batching (crates/holon-loro), CRDT payload size, and sync compatibility — frontier work.
- DONE gate: keystone green; `reset`+rebuild produces identical answers for the provable
  subset (test); fidelity reported matches implemented guarantee.
- Tier: minimum = **executor**; full = **frontier + Martin ruling**. Recommend shipping minimum
  and parking full in the vault parking lot with a slug ID.

### INC 5 (adjacent, recommended next after C2 proper) — ADR 0025 recovery grounding
- Record sync-origin deltas at the consolidator chokepoint (consolidator.rs:125-127) so deletes
  from ALL origins are grounded; dedup vs engine-recorded ops (R5); then feed
  `re_render_all_tracked` / mass-truncation tripwire from the relation
  (file_sync_controller.rs:2607,3203). This is ADR 0025's named follow-up, not C2 core —
  separate plan once C2 INC 1–3 are in.
- Tier: **frontier-reviewed executor** (block-loss territory; the seven-classes history says
  respect it).

Sequencing rationale (risk-elimination-first): INC 1 kills the two irreversible-if-wrong
unknowns (schema shape, write-path cost). INC 2 kills the one architectural unknown (IVM over
the history table). INC 3 is payoff with near-zero risk. INC 4/5 are the genuinely hard tails,
isolated so they can't block the payoff.

## 5. Architecture forks for Martin (recommendation marked; substance, not labels)

**F1 — Rule reactivity surface over history.**
(a) Matview(s) over `block_history` (INC 2): rules watch a maintained relation, CDC fires,
    O(delta) — consistent with how trust_proposals and advice already work; costs one matview
    per query shape (IVM can't maintain arbitrary ad-hoc queries).
(b) No matview; Guide rules poll via clock-tick read-arcs (C6 machinery): zero new IVM surface,
    but rules become periodic, not reactive, and every poll is O(scan).
**Recommendation (a)** — it is the existing house pattern and the only one where "Watcher
notices the 7th postponement immediately" is true. (b) remains available per-rule anyway.

**A2 — keystone provenance oracle: exact-equality REFUTED, subset + guarded lower bound landed.**
The INC 2 oracle was specified as an exact count-equality ("model-applied mutating ops ==
`block_history` op_groups, per case"). Three findings refute it as universally true, so it is
NOT what shipped:
(1) history is recorded ONLY on the engine `execute_operation` path (operation_engine.rs:598,899);
    the sync/Loro path wires `DegradedHistoryStore` (loro_block_query_source.rs:198) — a no-op —
    so peer/sync mutations record zero op_groups while the ref applies a mutation (F3(a)'s
    disclosed scope). (2) `op_group` is `NOT NULL` (one per non-empty `record_batch`), and a
    zero-delta op records NO row, so editor-batched `TypeChars` / boundary no-op transitions have
    variable (0..N) cardinality per transition. (3) the keystone alphabet contains both classes
    (`PeerEdit`/`MergeFromPeer`/…​ and `TypeChars`/`DeleteBackward`/edge no-ops). An exact
    equality would false-RED the keystone. Landed instead (both cap-gated on `SutHistory`,
    checked at settle): a phantom-history SUBSET check and a missed-history op-group LOWER BOUND
    over UI-driven creates (see INC 2). The full quantitative count-delta is PARKED (needs a
    per-variant `records_history` classifier through the transition-dispatch macro).

**F2 — Rebuild fidelity (INC 4).**
(a) Full: provenance rides Loro commit metadata → drop-anytime, rebuild-anywhere, true
    Fidelity::Loro. Cost: CRDT payload growth on EVERY commit, consolidator batching
    entanglement, cross-peer compat — the one genuinely frontier item in C2.
(b) Honest partial (recommended for now): rebuild recovers structure + create-provenance;
    fidelity reported as what it is; full fidelity parked with a vault slug.
(c) Status quo: constructor asserts Loro fidelity with no rebuilder — rejected: it's an
    undisclosed over-claim, exactly what the fail-loud philosophy forbids.

**F3 — Sync/ingest origin coverage.**
(a) v1 disclosed scope = engine-dispatched ops only (module docs say so; queries against
    origins {user,agent,rule} are complete, `sync` absent) — zero risk now.
(b) Record at consolidator too — needed for ADR 0025 grounding (Q6) but introduces the
    double-record dedup problem (R5).
**Recommendation (a) for C2, (b) deliberately deferred to INC 5** with the dedup design done
at plan time, not patch time.

## 6. Risk register

| id | increment | risk | mitigation |
|---|---|---|---|
| R1 | INC1 | history INSERTs on dispatch critical path breach latency SLO at vault scale | batch per op in one txn; measure via latency_e2e + holon_latency before/after; acceptance <5ms p95; if breached: move append off-path behind a channel with fail-loud backpressure (disclosed ordering caveat) |
| R2 | INC2 | fork IVM can't maintain GROUP BY-over-table matview or hangs (cf. turso-chained-matview-hang skill; memory says chained supported — conflicting signals) | spike FIRST with a minimal repro in crates/holon/tests/turso_storage_repros style; fork is ours (turso-ivm-ours) — extend if needed via turso-fix skill; fallback: plain view + poll for v1, disclosed |
| R3 | INC1 | schema drop+recreate on version bump surprises a running peer/session | version tag in DDL + boot-time reconcile mirrors matview reconcile pattern; relation is disclosed-ephemeral so loss is contractually fine |
| R4 | INC1–3 | unbounded table growth at vault scale degrades queries | indexes already exist (history_store.rs:71); measure at vault scale in INC1; retention deferred + disclosed (non-goal) |
| R5 | INC5 | double-recording when consolidator-recorded sync deltas overlap engine-recorded local ops | design before code: local ops tagged with op_group at dispatch; consolidator records only deltas whose origin peer ≠ self (Loro peer-id available in DiffEvent); INC5 gets its own plan |
| R6 | INC2 | keystone oracle flaky under settle/timing (cf. sibling-order-flaky history) | count-based invariant checked at quiescence points only; reuse combined-fixed-point settle |
| R7 | all | docs (VisionGap, memory) stale vs tree — implementers re-derive wrong premises | every increment PR updates VisionGapAnalysis C2 row + this plan's §0 evidence lines |

## 7. Evidence appendix (staleness guard — re-verify with these greps)

- `grep -rn "trait HistoryStore" crates` → holon-api/src/history.rs:131
- `grep -rn "with_history_store" crates` → operation_engine.rs:266; backend_engine.rs:121,803
- `grep -n "record_history" crates/holon/src/api/operation_engine.rs` → :356 def; :578, :879 callsites
- `grep -n "block_history" crates/holon/src/api/history_store.rs` → DDL/table name
- `grep -rn "HistoryQuery" crates frontends --include=*.rs | grep -v holon-api | grep -v history_store | grep -v operation_engine` → EMPTY = G1 still true
- `grep -rn "DegradedHistoryStore::new" crates` → EMPTY = G8 still true
- `grep -n "reconcile_named_view" crates/holon-turso/src/matview_manager.rs` → :59-90
- `grep -n "TRUST_PROPOSAL_STATS_SQL" crates/holon-turso/src/schema_modules.rs` → :426
- `grep -n "C2b" docs/adr/0025-op-grounded-projections.md crates/holon-filesystem/src/file_sync_controller.rs crates/holon-orgmode/src/writeback_guard.rs`
- `grep -n "execute_batch_with_origin" crates/holon-loro/src/consolidator.rs` → :125-127
