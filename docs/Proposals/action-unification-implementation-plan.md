# ADR 0024 Phases 1–2 — Implementation Plan

**Status:** Draft for senior review (2026-07-09). Structured for review: numbered
work packages, explicit global assumptions, open-questions section at the end.
**Basis:** [ADR 0024](../adr/0024-unified-action-execution.md) (incl. the
Amendment "effects are token operations" and the revised guard-surface
paragraph), [action-ux.md](action-ux.md) (the UX contract), and
[ADR 0022](../adr/0022-runtime-definable-advice-rules.md) (the reusable
rules-as-blocks → synthesized-matview → anti-join machinery).
**Scope:** ADR 0024 **Phase 1** (three independent WPs) and **Phase 2** (a
de-risking spike then the Pattern-AST promotion). Phases 3–5 are horizon markers
only (§8), awaiting ADR ratification.

---

## Status (2026-07-15)

**PARTIALLY LANDED.** Phase 1 (clock/effect-id/program-marking) is landed:
`ClockSchedulerHandle` (`crates/holon/src/sync/clock_scheduler.rs`),
`is_program` flag (`crates/holon-profiles/src/lib.rs`), `holon_rule` source
language (`crates/holon-api/src/types.rs`), `Pattern` enum
(`crates/holon-api/src/pattern.rs`), and the `holon-advice` crate. Phase 2
spike + full promotion WP (grammar, parser, `action_watcher` consuming
Pattern-compiled matviews, provenance stamping — §7.2) are not yet shipped
(possibly WIP uncommitted). Phases 3–5 remain horizon markers.

Still open:
- Phase 2 full promotion WP (§7.2)
- Phases 3–5 (unified rule+advice format, nets, deliberation)
- Keystone `AdvanceDay` capstone transition

---

## 0. Executive summary

ADR 0024 says the three "when-condition-then-effect" machines converge onto one
substrate (blocks) with one semantics (a dual-evaluated Pattern AST). Phases 1–2
land the **decision-invariant** groundwork that is correct under every ratified
outcome, plus the guard-language spine everything later depends on.

- **Phase 1 is three genuinely independent WPs** (different files, no shared
  types) that can run as three parallel workstreams:
  - **WP1 time-as-data** — a projected `clock` relation advanced by a
    `ClockScheduler`; temporal triggers become ordinary reactive matview
    triggers; the `is_tableless` boot-one-shot branch of `action_watcher` is
    **deleted entirely** (no legacy path).
  - **WP2 deterministic effect IDs** — rule-fired `block.create` mints a
    name-based (UUIDv5) id from `rule-id + firing-key`, so two replicas firing
    the same rule for the same key converge to one block. Subsumes the
    `create: missing 'id'` panic (BugFunnel F1).
  - **WP3 program marking** — a new `holon_rule` source language (superseding
    the bare `action` language) plus a derived `is_program` flag route rule
    blocks (and their paired trigger blocks) to a **rule-card** render path
    instead of the broken query-result path (BugFunnel F2/F4 render half).
- **Phase 1 capstone (sequenced after the three WPs):** a keystone PBT
  `AdvanceDay` transition that ticks the clock and asserts *exactly one* journal
  per day under settle — the prod hypothesis that WP1+WP2+WP3 compose.
- **Phase 2 begins with a spike** that promotes a minimal Pattern AST unifying
  `holon_api::Predicate` (scalar, prod) with the PBT `query_ast` relational
  vocabulary (`PropEq`/`Membership`/`EdgeExists` + bindings), dual-evaluated
  `evaluate()`/`to_sql()` against a projection-owned schema abstraction, carrying
  the existing in-memory ≡ SQL agreement oracle into prod as a pinned invariant.
  **Exit criterion:** the journal guard `not block_exists("Journals/{today}")`
  (with `{today}` desugared to a clock read-arc) round-trips through both
  evaluators with agreeing results. Then the full promotion WP wires the parser,
  grammar, and `action_watcher` onto Pattern-compiled matviews with provenance
  stamping.

**Reviewer rulings incorporated (2026-07-09):** Q1–Q5 resolved (§9), plus five
new findings folded in — the clock-injection race (WP1/§6), loud `action`
retirement (WP3), embedder parity for the scheduler (WP1), the Phase-1→2
firing-key migration safety (§7.2), and both-pins capstone scope (§6). Remaining
judgment sits in the ruled decisions, now marked RESOLVED inline.

---

## 1. Global assumptions

- **A1 — Phase 1 keeps today's query+action *pair* shape.** The single-block
  `holon_rule` YAML (guard-as-string) is Phase 2/3 grammar. Phase 1 renames the
  `action` block's language to `holon_rule` and hides both blocks of the pair,
  but does **not** yet collapse the pair. (Challenged in §9-Q2.)
- **A2 — PR #33 provider minting is NOT present in this base.** The scout found
  `sql_operation_provider.rs:1119` still `.expect("create: missing 'id'")`.
  WP2 therefore *implements* provider-side minting (random v4 for generic
  callers) AND the rule-path deterministic v5 override. If PR #33 has since
  merged, WP2 rebases onto it and only adds the deterministic override.
- **A3 — the `clock` is an evaluator detail of the reactive path, not
  semantics** (RESOLVED, see §9-Q1): `{today}` denotes *ambient time*; the
  reactive evaluator desugars it to a CDC-eligible read arc on the `clock`
  relation, while Phase 4's in-memory evaluator desugars the **same** builtin to
  a direct `Clock::now()` call — so there is no standalone-evaluator gap. The
  relation is a **cache of the OS clock** (the scheduler re-seeds it at boot),
  never authoritative — P1a untouched. It does not route through the
  consolidator/Loro and is not a block; per-replica independent advance is
  harmless because internal effects are convergent (ADR P4/P5).
- **A4 — projection is total** (ADR P1b): every rule and every trigger row is
  present in Turso when the app pipeline runs, so reactive evaluation via
  matview + CDC is the Phase-1/2 evaluator; the in-memory evaluator is Phase 4.
- **A5 — no legacy paths** (repo rule): each deletion is part of its WP; we do
  not keep the one-shot branch, the bare `action` language, or the
  string-matched discovery "just in case."
- **A6 — fail loud:** every parse/compile/DDL failure surfaces on the rule card
  via a status handle (the ADR 0022 `AdviceRuleStatusHandle` pattern), never a
  swallowed `.ok()`.

---

## 2. Phase 1 — parallelization map

```
        WP1 time-as-data ─┐
        WP2 det. IDs ─────┼──▶ Phase-1 capstone: keystone AdvanceDay transition
        WP3 prog. mark ───┘        (depends on all three)

   WP1 ──────────────────────▶ Phase 2 SPIKE (needs the clock relation for {today})
```

WP1/WP2/WP3 touch disjoint files (WP1: turso schema + a new scheduler + delete
one `action_watcher` branch; WP2: `sql_operation_provider` + `fire_action` + a
new id module; WP3: `types.rs` SourceLanguage + `block_profile.yaml` + a derived
matview + `Journals.org` + `action_discovery.sql`). They share no new types.
Assign to three workstreams. The capstone and Phase 2 are sequenced.

---

## 3. WP1 — Time as data (clock relation + scheduler; delete `is_tableless`)

**Goal.** Make temporal guards ordinary reactive matview triggers by introducing
a deterministic `clock` relation that carries the materialized `today` value, and
delete the boot-one-shot `is_tableless` branch of `action_watcher`.

**Why (ADR principle).** P5 "Time is data": `date('now')` is non-deterministic so
Turso rejects it as a matview source (BugFunnel F4); a `today` *value* in a table
is deterministic and CDC-observable, so a temporal guard is a plain join that
re-fires on day-rollover. Deleting the one-shot branch removes the "temporal
triggers never re-fire" defect and the whole tableless special case.

**Files touched.**
- **New** `crates/holon-turso/sql/schema/clock.sql` — the `clock` base table
  DDL. Registered by extending `CoreSchemaModule` in
  `crates/holon-turso/src/schema_modules.rs:51` (the `include_str!` list) and
  `crates/holon/src/di/schema_providers.rs:134` (`register_schema_providers`).
- **New** `crates/holon/src/sync/clock_scheduler.rs` — the `ClockScheduler`
  actor (mirrors `advice_reconciler.rs`'s own-a-`DbHandle` + drainer pattern),
  spawned from `crates/holon/src/di/registration.rs` alongside
  `spawn_advice_reconciler` (near line 127).
- **New newtype** in `crates/holon-api/src/clock.rs` (extends the existing
  `Clock`/`SystemClock` trait at lines 6–19) — `CalendarDate` and `Grain`.
- **Delete** the `is_tableless` branch of `run_pair_watcher_inner`
  (`crates/holon/src/api/action_watcher.rs:187–202`) *and* the now-dead
  `parse_sql`/`extract_table_refs` call. The tableless helpers in
  `crates/holon/src/storage/` lose their only action-watcher caller — delete
  them if no other caller remains (scout to confirm blast radius via
  `ast-outline reverse-deps`).
- **Migrate** the journal trigger (currently `SELECT date('now','localtime')`,
  `assets/default/Journals.org:12–14`) to join the clock relation, e.g.
  `SELECT today FROM clock WHERE grain = 'day'` (final SQL depends on the pair
  shape; the point is it is now table-backed and CDC-eligible).

**New types (parse-don't-validate).**
- `Grain` enum `{ Day }` (`Hour`/`Minute` reserved; no stringly grain).
- `CalendarDate(String)` newtype — constructed only via a parser that validates
  `YYYY-MM-DD`; the scheduler and any `{today}` desugaring produce/consume it,
  never a bare `String`.
- `clock` schema: `grain TEXT PRIMARY KEY, today TEXT NOT NULL, epoch_day
  INTEGER NOT NULL, updated_at TEXT NOT NULL`. Single row per grain. `epoch_day`
  gives temporal guards a monotone integer to compare without date parsing in
  SQL.

**Scheduler semantics.** The `ClockScheduler` takes the existing `Clock` trait
(`holon-api/src/clock.rs`) **via DI** — it never calls the OS clock directly. On
boot, seed the `day` row from `Clock::now()`. Tick on a `tokio::time::interval`
(e.g. 30 s — cheap; the write only happens on change); each tick computes the
local `CalendarDate` from the injected clock; if it differs from the stored row,
issue an `UPDATE clock SET today=…, epoch_day=…, updated_at=…` via the owned
`DbHandle` (a **direct projection write**, not a block intent — A3). The CDC that
`UPDATE` emits is what re-fires every temporal guard's matview. Per-replica
independent ticking is correct (A3/P5).

**Clock can go *backwards*** (DST fall-back, timezone travel west): the scheduler
writes on **any** change, not only forward, and deterministic effect IDs (WP2)
make the resulting re-fire converge. A unit test must cover a backwards day
change.

**Injection is what makes the capstone possible (no clock race).** Because the
scheduler reads the injected `Clock`, the keystone `AdvanceDay` transition (§6)
advances a **fake `Clock`** and lets the scheduler propagate the new day through
the real prod path — it must **never** raw-`UPDATE` the `clock` relation behind
the scheduler's back (the scheduler would immediately overwrite the injected day
with the real wall-clock date).

**Test strategy.**
- `holon-turso` schema unit test: `clock.sql` creates, seeds one row, an
  `UPDATE` emits CDC.
- `holon` lib test for `ClockScheduler`: inject a fake `Clock` that jumps a day;
  assert the row advances and exactly one CDC `Updated` fires. **Plus a
  backwards-day-change test** (fake clock moves the date earlier — DST/travel).
- The **keystone capstone** (§6) is the real integration proof (day-rollover
  re-fires the journal rule).

**Embedder parity (ENVIRONMENT is the top BugFunnel escape category).** The
`ClockScheduler` must be spawned in **every** embedder wiring — GPUI desktop,
iOS, dioxus-web worker, headless test — or the app fails loud at boot. This is
covered by spawning it in the shared `crates/holon/src/di/registration.rs`
(the same `create_initialized_engine` path all embedders resolve through, per
scout). WP1 adds a **boot guard/assertion** that the `clock` row is seeded and a
scheduler handle is live; any embedder that bypasses `registration.rs` must be
listed and fixed (none known — verify during implementation).

**Done-criteria.** `is_tableless` branch gone; journal trigger is table-backed;
a simulated day-rollover re-fires the temporal guard (proven in the capstone); a
backwards-day change converges; no `date('now')` remains in any matview source;
the boot guard fires if any embedder starts without a live scheduler. BugFunnel
F4 (matview half) closed.

**Size:** M. **Depends on:** none. **Blocks:** capstone, Phase-2 spike.

---

## 4. WP2 — Deterministic effect IDs

**Goal.** Rule-fired `block.create` mints a name-based UUID (UUIDv5 of
`rule-id + firing-key`) so concurrent replicas firing the same rule for the same
key produce the *same* block id; the tree merge then collapses them.
At-most-once-per-key becomes a naming discipline, not an execution-semantics
problem.

**Why (ADR principle).** P4 "internal effects converge by construction via
deterministic effect IDs." Also subsumes the `create: missing 'id'` panic
(BugFunnel F1, PR #33 lineage): the rule path always supplies an id.

**Where the id is computed — the load-bearing decision.** The deterministic id
must be minted **where the rule context (rule-id + firing key) is known** — i.e.
in `fire_action` (`crates/holon/src/api/action_watcher.rs:224`), the effect
compilation site — **not** in the generic `SqlOperationProvider::execute_operation`
create case, which serves all callers and has no firing key. The generic
provider keeps a random-v4 fallback for id-less callers (the PR #33 behaviour);
the rule path passes an explicit deterministic id so the fallback never triggers
for rules.

**Files touched.**
- **New** `crates/holon-api/src/effect_id.rs` — `deterministic_block_id(rule:
  &RuleId, key: &FiringKey, slot: &OutputSlot) -> EntityUri`. The `OutputSlot`
  discriminator (Martin's ADR review, P4): IDs are minted **per emitted
  token** — a transition with N output arcs (e.g. a today-page template
  creating several blocks) mints N distinct ids
  `UUIDv5(ns, rule ‖ key ‖ slot ‖ [index])`, never one shared id. Phase 1's
  single-create rules pass a fixed slot, but the signature carries it from day
  one so Phase-2 templates need no migration. Uses `uuid::Uuid::new_v5` (dep
  already present in `holon-api/Cargo.toml:24`) with a fixed Holon namespace
  UUID constant; wraps via `EntityUri::block(...)` (`entity_uri.rs:65`) so the
  scheme prefix is correct.
- `crates/holon/src/api/action_watcher.rs` — `fire_action` computes the
  `FiringKey` from the produced trigger row and, when the operation is `create`,
  inserts the deterministic id into `params` before `execute_operation`.
- `crates/holon/src/core/sql_operation_provider.rs:1119` — replace the
  `.expect("create: missing 'id'")` with fail-loud minting: if `id` present use
  it; else mint `EntityUri::block(Uuid::new_v4())`. (This is the generic
  fallback; A2.)

**New types (parse-don't-validate).**
- `RuleId(String)` newtype (= the discovery `action_id`; parsed once at the
  discovery boundary, threaded through `run_pair_watcher`).
- `FiringKey(String)` newtype — a **canonical** serialization of the produced
  trigger row (sorted `key=value` pairs). Convergent across replicas because
  projection is total (A4) and the row is derived identically on each replica.
  Constructed only via `FiringKey::from_row(&StorageEntity)`; never a bare
  string. (Phase 2 replaces this with the explicit `emit` key / interpolated
  builtins — §7; the newtype boundary makes that swap local.)
- Namespace: `const HOLON_RULE_NAMESPACE: Uuid` (a fixed, checked-in v5
  namespace).

**Test strategy.**
- `holon-api` unit test: `deterministic_block_id` is stable across calls and
  distinct across rule/key/slot (incl. two slots of one firing → two ids).
- **`holon-loro-testing` PBT (the at-most-once-under-concurrent-replicas
  property).** Two `LoroSut` replicas (`sut_loro.rs:36`, `apply_add_peer`) each
  fire the same journal rule for the same day independently, then
  `apply_merge_from_peer` + quiescence (`quiescence.rs:15`); assert the merged
  doc has **exactly one** journal block for that day. This is the prod
  hypothesis that deterministic ids give at-most-once under concurrency — the
  property that no execution log could provide (ADR P4). No existing two-peer
  convergence property exists (scout confirmed) — this is a new harness/test.

**Done-criteria.** Rule-fired creates carry a deterministic id; two-replica
concurrent firing converges to one block (loro PBT green); the `create: missing
'id'` panic is gone (generic fallback mints). BugFunnel F1 closed.

**Caveat — protection is `create`-only in Phase 1.** Deterministic-id convergence
gives at-most-once *only for `create` effects*. Under re-fire (boot
re-registration, day rollover, and especially **projection resync**, which
Replace-recovers by re-emitting every trigger row as `Created`), non-create
effects (`set_field`/`update`/`delete`) **re-execute** — a pre-existing
`action_watcher` defect, disclosed here (Q3). Non-create re-fire safety is
deferred to Phase 2's anti-join/inhibitor guards; Phase 1 does not claim it.

**Size:** M. **Depends on:** none (for the unit path). The loro PBT capstone
shares infra with §6 but is independent of WP1/WP3.

---

## 5. WP3 — Program marking (rule-card render, not broken query)

**Goal.** Rule/action blocks (and their paired trigger blocks) are excluded from
content rendering and routed to a rule-card render path, fixing the
render-as-broken-query bug.

**Why (ADR principle).** P6 "program is data, but not display content." The
journal-rule render bug (BugFunnel F2 literal-text rows / F4 blank panel) is
exactly this marking missing. UX contract: the rule becomes the *most* legible
block on the page — name, enabled toggle, `last fired`, and a fail-loud red error
state (action-ux.md §"Rendering: the rule card").

**Validation task (do before implementation — repo hazard).** Confirm the
renderer's row source can carry `is_program` **without** a
matview-reading-a-matview (the chained-matview hang;
`.claude/skills/turso-chained-matview-hang`). If the row pipeline's source is
already a matview, `is_program` must be a **column/join inside that same view**,
not a stacked one. This gates the schema shape below.

**Files touched.**
- `crates/holon-api/src/types.rs:378–382` — add `SourceLanguage::HolonRule`
  variant; update `FromStr` (line 407) so `"holon_rule"` parses to it. **`"action"`
  parses to a distinct `SourceLanguage::LegacyAction` (deprecation) sentinel —
  NOT dropped and NOT folded into `Other`** (see loud-retirement below).
- `assets/default/Journals.org:15–17` — migrate the action block's language
  `action → holon_rule` (the seed-asset migration).
- `assets/queries/action_discovery.sql:14` — discovery matches
  `source_language = 'holon_rule'` (was `'action'`).
- **New derived `is_program` flag** — a **column/join in the renderer's existing
  row source view** (per the validation task; not a stacked matview): flags
  (a) any block with `source_language='holon_rule'` and (b) any source block that
  is the **trigger sibling** of such a block (reuse the `action_discovery.sql`
  parent-sibling join so the renderer never runs discovery). Crux design choice
  — §9-Q2 (RESOLVED).
- `assets/default/types/block_profile.yaml` — (i) gate the generic `source`
  variant (lines 67–70, `query_result`) on `not is_program`; (ii) render program
  blocks via a **`rule_card`** variant (rule name, enabled flag, last-fired,
  status/error). **Reconcile the `is_holon_source` collision (Q2):** line 20's
  `is_holon_source: source_language.starts_with("holon_")` currently makes any
  `holon_*` block match the dead `holon_source` spacer variant (line 51). WP3
  **replaces that stringly `starts_with` predicate** (exactly the banned
  `match str` smell) with the typed `is_program` flag / an explicit
  `SourceLanguage` check, so precedence is *documented in types*, not implied by
  variant ordering. The `holon_source` spacer variant is retired.
- **New status surface** reusing the ADR 0022 pattern: a `RuleStatusHandle`
  (Arc<RwLock<HashMap<block_id, RuleStatus>>>) analogous to
  `holon-advice/src/status.rs:59`, populated by the watcher on
  parse/compile/exec failure; the rule card renders it red (fail-loud, A6).

**Loud retirement of the `action` language (A5, no silent degradation).** Real
user vaults (`holon-pkm`) contain `action` blocks; silently mapping them to
`Other("action")` = dead rules = banned silent degradation. Discovery/watcher
treats `source_language='action'` as an **explicit deprecation error** surfaced
on the rule card via the status handle (`RuleStatus::DeprecatedLanguage` —
"legacy 'action' language; rename to holon_rule"), in addition to the seed-asset
migration. The rule is visibly broken, never silently inert.

**New types (parse-don't-validate).**
- `SourceLanguage::HolonRule` and `SourceLanguage::LegacyAction` (enum variants;
  the latter is a typed deprecation sentinel so `action` fails loud, never falls
  into the `Other(String)` bucket).
- `RuleStatus` enum `{ Active, ParseError(String), CompileError(String),
  ExecError(String), DeprecatedLanguage }` (mirrors `AdviceRuleStatus` at
  `status.rs:17`, plus the deprecation variant).

**Test strategy.**
- `holon-frontend` lib test on the render interpreter (`pick_active_variant`,
  `render_interpreter.rs:713`): a `holon_rule` block picks the `rule_card`
  variant and a paired trigger block picks it too (both `is_program`), and
  neither picks `query_result`.
- A `holon-api` unit test for `SourceLanguage` round-trip incl. `holon_rule`.
- Manual/MCP dogfood confirmation the seeded Journals page no longer shows
  literal-text machinery rows (BugFunnel F2/F4 render half).

**Done-criteria.** No rule/trigger block renders as a query result; the rule
card renders name + enabled + last-fired + fail-loud error; the stringly
`is_holon_source` predicate is replaced by a typed check; a legacy `action` block
surfaces a loud `DeprecatedLanguage` status (never silently inert); the
seed-asset is migrated to `holon_rule`. BugFunnel F2 (render half) and F4
(blank-panel render half) closed. (Dup-Journals seeding — F2's other half — is a
separate worker-model root cause, explicitly deferred here.)

**Size:** M–L (the derived `is_program` matview and the card variant carry the
weight). **Depends on:** none.

---

## 6. Phase-1 capstone — keystone rule-firing transition

**Goal.** Encode the prod hypothesis that WP1+WP2+WP3 compose: advancing the
clock re-fires the journal rule and yields *exactly one* journal per day,
idempotent under repeated ticks. (CLAUDE.md rule: the keystone should gain a
rule-firing transition when Phase 1 lands.)

**Design.** A new transition file
`crates/holon-integration-tests/src/pbt/transitions/advance_day.rs` following the
established anatomy (struct + `TransitionFactory<R>` + `TransitionRef<R>` +
`cap_transition!`), registered as one variant in `declare_e2e_transitions!`
(`transitions/mod.rs:192`). The SUT cap advances the **injected fake `Clock`**
(never a raw `UPDATE` of the `clock` relation — WP1's clock-injection race) and
lets the `ClockScheduler` propagate the new day through the prod path;
`apply_to_ref` advances a reference-model `today` and, in the oracle, asserts the
journal set gains exactly one entry per new day. Settle is automatic — the
harness calls `S::settle_after_apply` (`composed/harness.rs:361`) →
`converge_projections` (`wide_e2e.rs:120`) before invariants run, so the reactive
rule firing projects through first.

**Invariant.** After N `AdvanceDay` transitions spanning D distinct days, the
projection holds exactly D journal blocks (one per day), each with the
deterministic id (WP2), none rendered as program content (WP3). **Re-ticking the
same day (and a projection resync) adds nothing *for creates*** — the
deterministic id converges (Phase 1 makes no at-most-once claim for non-create
effects; Q3).

**Modes.** The capstone runs under **both** keystone pins — `Loro;;UI` and
`SqlOnly` — covering **single-replica idempotence in both modes**. The
concurrency half (at-most-once under two independent replicas firing the same
key) is the separate `holon-loro-testing` PBT in WP2 (Loro-only, since it
exercises peer merge). Together they cover single-replica idempotence (both
modes) + multi-replica convergence (Loro).

**Test strategy / done-criteria.** Keystone green with the new transition in the
mix under both pins. This is the single composed PBT that must reproduce any
journal-rule regression (CLAUDE.md rule 1).

**Size:** M. **Depends on:** WP1, WP2, WP3.

---

## 7. Phase 2 — the Pattern AST

Sequenced after Phase 1. Starts with a spike whose sole job is to de-risk the
unification before the full promotion.

### 7.1 SPIKE — promote a minimal dual-evaluated Pattern AST

**Goal.** Prove that one `Pattern` type can carry both `evaluate()` (in-memory)
and `to_sql()` (matview) against a projection-owned schema abstraction, with the
in-memory ≡ SQL agreement oracle promoted from the PBT into prod as a pinned
invariant.

**Why (ADR principle).** The guard-language decision: "a dual-evaluated Pattern
AST, not Rhai." The agreement oracle already exists in `query_ast.rs`
(`now_query_compiles_to_canonical_sql` + `evaluate_now_query`); promoting the AST
carries the oracle along, and designing to the IVM-supported subset makes
unsupportable guards fail at *parse*, not at matview-DDL time (the `date('now')`
failure class disappears).

**What exists to unify.**
- `holon_api::Predicate` (`crates/holon-api/src/predicate.rs`): **scalar-only**
  (`Var/Eq/Ne/Gt/Lt/…/And/Or/Not/Always`), serializable, `evaluate()` over
  `HashMap<String,Value>`, `flutter_rust_bridge:non_opaque` (crosses FFI). No
  SQL, no relational, no bindings.
- PBT `query_ast::Predicate`
  (`crates/holon-integration-tests/src/pbt/query_ast.rs:88`): **relational** —
  `PropEq/PropNe/Membership{negated,edge}/EdgeExists{negated,edge,inner}` with an
  `Alias` (Outer/EdgeTarget) for one level of correlation, both `pred_to_sql`
  (188) and `evaluate` (432, returns matched `Vec<EntityUri>` = the bindings).
  **Caveat (ADR):** `pred_to_sql`/`json_extract` hardcode schema shapes
  (`json_extract` on properties, `block_tags`, `block_requires`).

**Spike deliverable.** A **new prod module** `crates/holon-api/src/pattern.rs`
(kept distinct from the FFI-crossing scalar `Predicate` to avoid perturbing the
non-opaque bridge — see §9-Q4) holding a `Pattern` enum that lifts the *design*
of the relational vocabulary but targets a **`SchemaAbstraction` trait owned by
the projection** instead of hardcoded `json_extract`/`block_tags`. Implement
`evaluate(&self, ctx, &dyn SchemaAbstraction)` and `to_sql(&self, &dyn
SchemaAbstraction)`. Port the agreement test as a prod PBT.

**Exit criterion (hard gate).** The journal guard
`not block_exists("Journals/{today}")` — with `{today}` desugared to a **read arc
on the `clock` relation** (WP1) and `block_exists(path)` desugared to a
path-existence `EdgeExists`/`PropEq` — round-trips through **both** `evaluate()`
and `to_sql()` and the two agree on a seeded fixture (journal present ⇒ both
false; absent ⇒ both true). If this cannot be made to agree, stop and escalate:
the Pattern AST shape is wrong before any parser is built.

**Size:** M (spike — throwaway-tolerant, but the module it lands is kept if the
gate passes). **Depends on:** WP1 (the clock relation for `{today}`).

### 7.2 Full promotion WP

**Goal.** With the AST proven, wire the `holon_rule` YAML grammar, a
parse-don't-validate parser, `action_watcher` consuming Pattern-compiled
matviews, and provenance stamping.

**Why (ADR principle).** P3 "one user-facing rule language"; Amendment "effects
are token operations / builtins interpolate"; P8 "firing history is provenance."

**Scope.**
- **Grammar** (`holon_rule`, valid YAML): `when:` (guard string parsed by the
  Pattern parser), `emit:`/`consume:` marking deltas. **Builtin interpolation**
  `{today}`/`{clock.today}` are environment references the *compiler* desugars
  into read arcs on the clock relation (not pattern variables — no `as` binding).
  `block_exists("Journals/{today}")` path sugar. **Placement kind** lives in the
  `emit.place` value (the axis `display(under: x)` sits on): `place: <root>` = an
  inline child of `block:<root>`; `place: page(<root>)` = a page-file child
  (`Page`-tagged → materializes to its own `<name-chain>.org` via the fileless-page
  sweep — the journal `Journals/{today}.org` intent). Grammar + watcher LANDED; the
  default seed flip to `page(journals)` is deferred to Fork B B1 (companion
  de-inline — a rule-created child page is otherwise inlined into the `Journals.org`
  companion, `inv-companion-has-no-child-page-headings` /
  `inv-sidebar-page-tag-preserved` red-first pending it). Mirrors
  `holon_advice_rule_yaml` (one authoring family, effect kinds `advise` |
  `operate`, per the ADR — `operate` is this WP; `advise` stays ADR 0022).
- **Parser** — model on `holon-advice/src/rule.rs` (`parse_advice_rule` +
  `AdviceRuleParseError` typed error enum + newtypes). Produce a typed `Rule`
  (`RuleId`, `Guard(Pattern)`, `Emit`/`Consume` marking deltas) — no stringly
  interfaces. Well-formedness: the Datalog range-restriction check surfaces only
  for user-introduced pattern variables (future quantifiers), never for builtins
  (ADR Amendment).
- **Compile + register** — model on `holon-advice`
  (`lowering.rs`/`synthesis.rs`/`reconcile_plan.rs`/`sync/advice_reconciler.rs`):
  the guard Pattern lowers via `to_sql` to a synthesized per-rule matview;
  `Change::Created` on it fires the effect via `execute_operation`. Reconciler
  is the pure `plan()` + async `apply_plan()` shape (create/drop matviews as rule
  blocks change), reusing `matview_manager::reconcile_named_view`.
- **`action_watcher` re-point** — `run_pair_watcher_inner`
  (`action_watcher.rs:157`) stops compiling a free-form query string
  (`compile_to_sql`) + Rhai action DSL, and instead consumes the
  Pattern-compiled matview; the Rhai `action_dsl.rs` matching path is retired
  (Rhai's only residual role is *effect* value construction, never matching —
  ADR).
- **Provenance stamping** — engine-executed ops carry `fired-by: <rule-id>` (the
  Model.md "serializable ops with provenance" kept-warm slot, ADR P8); the
  automation journal is then a query over provenance-stamped effects, not a
  stored log.

**New types.** `Rule`, `Guard(Pattern)`, `MarkingDelta { emit: Vec<Emit>,
consume: Vec<Consume> }`, `BuiltinRef { Today }` (desugars to a clock read-arc),
`RuleParseError` (typed, mirrors `AdviceRuleParseError`), `Provenance {
fired_by: RuleId }`.

**Firing-key migration is safe across the Phase-1→2 boundary.** Phase 2 switches
the `FiringKey` from the row-hash to the explicit `emit` name
(`"Journals/{today}"`), which changes the deterministic ids of *future* creates.
This needs **no backfill and is safe** because: (1) already-created journal
blocks keep their Phase-1 ids untouched; (2) at-most-once across the boundary is
now guarded by **pattern existence** — the `not block_exists("Journals/{today}")`
anti-join/inhibitor — not by id equality, so a day that already has a journal
(under either id scheme) does not re-fire regardless of which key the new create
*would* have used. Id equality stops being the sole dedup mechanism exactly when
the anti-join arrives.

**Test strategy.** The promoted agreement PBT (7.1) stays green as the invariant.
Parser round-trip unit tests. The keystone `AdvanceDay` transition (§6) now
drives the *Pattern-compiled* path instead of the query+action pair (the pair
collapses to one `holon_rule` block — A1 resolves here). loro at-most-once PBT
(WP2) re-runs against the new firing key (now the explicit `emit` name /
interpolated `{today}`, replacing the row-hash `FiringKey`).

**Done-criteria.** A single `holon_rule` YAML block expresses the journal rule;
its guard is dual-evaluated with the agreement invariant pinned; firing stamps
provenance; `action_dsl.rs` matching + the query+action pair are deleted.
**Predicate/Pattern convergence commitment (Q4):** scalar `Predicate` becomes (or
is re-exported as) the scalar subset of `Pattern` — e.g. `Pattern::Scalar(Predicate)`
— **no third predicate-ish type appears**, and render-variant predicates keep
using the scalar subset unchanged. The FFI-driven split is retired as a
first-class convergence step, not left as permanent duplication.

**Size:** L. **Depends on:** 7.1 spike, WP1, WP2, WP3.

---

## 8. Out of scope — Phases 3–5 (horizon markers, await ADR ratification)

- **Phase 3 — unified rule+advice format.** One rule definition format +
  discovery/lifecycle generalizing ADR 0022, with effect kinds `advise` (view) |
  `operate` (transition) sharing one authoring model. The rich rule card,
  dry-run simulator, provenance badge, and Automations page (action-ux.md MVP)
  land here.
- **Phase 4 — nets.** Token-as-block markings (place = parent, token = child,
  consumption = `move_block`); transitions compile to matviews (reactive) and to
  the in-memory `holon-engine` evaluator (standalone); lease tokens for
  Once/Owner-only external effects.
- **Phase 5 — deliberation.** Simulator over serialized net subtrees;
  ranking/what-if as advice; the AlphaGo-shaped heuristic-guided search.

---

## 9. Open questions — RESOLVED by senior review (2026-07-09)

All five carry the reviewer's ruling inline; kept as a decision record.


**Q1 — Clock: projection-local relation or replicated block? RESOLVED —
projection-local.** The clock is an *evaluator detail of the reactive path*, not
semantics: `{today}` denotes ambient time; the reactive evaluator desugars it to
a CDC-eligible clock-relation read arc, while Phase 4's in-memory evaluator
desugars the SAME builtin to a direct `Clock::now()` call. There is therefore
**no standalone-evaluator gap** — the flagged risk dissolves. Ephemerality is
fine because the scheduler re-seeds the relation at boot; it is a *cache of the
OS clock*, never authoritative (P1a untouched). See A3.

**Q2 — How is the trigger-query block hidden in Phase 1? RESOLVED — option (a)**
(derive an `is_program` flag via the discovery join, gate `query_result` on
`not is_program`), with two additions folded into WP3 (§5):
- **Validation task before implementation** — confirm the renderer's row source
  can carry `is_program` *without* a matview-reading-a-matview (the chained
  matview hang is a known repo hazard —
  `.claude/skills/turso-chained-matview-hang`). If the row pipeline's source is
  already a matview, `is_program` must be a **column/join in that same view**,
  not a stacked one.
- **`is_holon_source` interaction** — `block_profile.yaml:20` defines
  `is_holon_source: source_language.starts_with("holon_")`, so naming the
  language `holon_rule` makes rule blocks silently match the existing
  `holon_source` spacer variant (line 51). WP3 resolves this explicitly (see §5)
  by **replacing the stringly `starts_with` predicate** — it is exactly the
  `match str` smell the repo bans — with a typed program check, rather than
  relying on undocumented variant precedence.

**Q3 — Phase-1 firing key = full-row hash? RESOLVED — accepted as a stopgap,**
with disclosures folded into WP2 (§4) and the capstone (§6):
- **Phase 1 has no anti-join guard.** At-most-once rests *entirely* on the
  deterministic id converging under re-fire — there is no inhibitor/anti-join
  until Phase 2.
- **Re-fire sources are enumerated:** boot re-registration, day rollover, and
  **projection resync** (Layer 4 recovery is *Replace*, so a resync re-emits
  every trigger row as `Created` and re-fires every reactive rule).
  Deterministic-id `create` effects converge (harmless); **non-create effects
  (`set_field`/`update`/`delete`) re-execute** — a pre-existing `action_watcher`
  defect. Phase-1 deterministic-id protection is therefore scoped **to `create`
  effects only**; non-create re-fire safety is explicitly deferred to Phase 2's
  anti-join/inhibitor guards.

**Q4 — New `Pattern` type vs. extending `Predicate`? RESOLVED — separate
`Pattern`, but with a convergence commitment.** The split is temporary FFI
prudence (the scalar `Predicate` is `flutter_rust_bridge:non_opaque`), **not** an
end state. 7.2 done-criteria (§7.2) commit: `Predicate` becomes (or is
re-exported as) the scalar subset of `Pattern` — e.g. `Pattern::Scalar(Predicate)`
— **no third predicate-ish type may appear**, and render-variant predicates keep
using the scalar subset unchanged.

**Q5 — `holon-engine` (ADR 0017) untouched until Phase 4. CONFIRMED.** Phases 1–2
build the reactive/matview path only; the in-memory evaluator and simulator are
Phase 4–5. Consistent with the ADR's staging (degenerate one-transition case
lands first; `action_watcher` is re-understood as its compiled output).
