# ADR 0022: Runtime-definable advice rules (typed rule blocks + engine-synthesized matviews)

**Status:** Accepted (2026-07-07). Decided via research + a hostile Fable review;
sub-decisions ratified by Martin the same day (see "Ratified sub-decisions"). This
ADR settles the last open advice fork left by [ADR 0021](0021-advice-suppression-storage-and-readonly-v1.md):
*where does the advice rule itself live, and who may author it?*
**Deciders:** Martin (+ adversarial Fable review)
**Relates to:** [ADR 0015](0015-computed-placement-and-curated-state-primitives.md)
(canonical/display placement; entity- vs element-identity §1a),
[ADR 0016](0016-occurrence-keyed-focus-authority.md) (occurrence-keyed focus),
[ADR 0021](0021-advice-suppression-storage-and-readonly-v1.md) (suppression storage +
read-only v1), `docs/Proposals/advice-feature-implementation-plan.md` (Increment F,
Spike-2).

## Context

Under the advice-as-query reframe (2026-07-07), advice is a read-only relevance
sub-region under an anchor block: advice results are **query rows** produced by a
relevance query, not an authored transclusion edge (ADR 0021). ADR 0021 fixed *where a
dismissal lives* and *that v1 renders read-only*. It left open the question this ADR
answers: **the advice rule** — the anchor selector + scoring + K that decides *which*
rows surface under *which* blocks — where does it live, and is it authored by the user
or hard-coded?

The precedent already in the tree is decisive. **Entity profiles** are user-editable
config blocks (`source_language = 'holon_entity_profile_yaml'`) discovered by a plain
SQL scan (`crates/holon/sql/profiles/get_profiles.sql`:
`SELECT id, content FROM block WHERE content_type = 'source' AND source_language =
'holon_entity_profile_yaml'`) and synced through org/Loro like any other block. Profiles
prove the mechanism class we need: **user-editable config, org/Loro-synced, injecting
queries into the render path**. Advice rules are a second instance of exactly that class.

## Decision — advice is runtime-definable via typed rule blocks with engine-synthesized matviews

An advice **rule is a vault block** with `source_language = 'holon_advice_rule_yaml'`,
discovered exactly like entity profiles (the `get_profiles.sql` pattern — a `content_type
= 'source'` scan keyed on `source_language`). Rules therefore round-trip org/Loro, are
authored in the vault, and are runtime-definable **day one** — no code change, no rules
engine, no restart.

### Parse boundary (parse-don't-validate)

The rule YAML is parsed at the boundary into a closed, typed representation with serde
`deny_unknown_fields`. Illegal states are refused at parse, never carried forward as
strings to be re-validated:

```
AdviceRule {
    name:       RuleSlug,          // → stable matview name  advice_rule_{slug}
    anchor:     AnchorSelector,    // SQL-lowerable typed predicate (NOT a scripting expr):
                                   //   Entity(EntityName) | HasTag(Tag)
                                   //   | PropEq(Key, Value) | And(Vec<AnchorSelector>)
    candidates: ScoringTemplate,   // CLOSED enum; v1 single variant:
                                   //   TagOverlapRecency { source, k }
    k:          BoundedK,          // 1..=10; parse REFUSES a larger K
    active:     bool,
    // + RESERVED versioned raw-query field (sub-decision 3): schema reserved
    //   day one; v1 parse REFUSES its use ("reserved, not yet supported")
}
```

**Refused at parse:** unknown fields (`deny_unknown_fields`), `K` over the cap
(`BoundedK` newtype, not a bare `usize`), any anchor predicate that is not
SQL-lowerable, and any use of the reserved raw-query field (sub-decision 3 — an
explicit "reserved, not yet supported" error, never a silent ignore). `AnchorSelector` is a **typed predicate that lowers to a SQL `WHERE`
clause**, deliberately *not* a scripting expression — this is what keeps the anchor set
IVM-computable rather than a per-commit full scan.

The **suppression anti-join** (`:ADVICE_SUPPRESSED:` drawer → `advice_suppressed`
LEFT JOIN … IS NULL, per ADR 0021 / Spike-2) is **implied by every rule and never
configurable**. A rule cannot opt out of honoring the user's dismissals.

### Engine owns DDL synthesis

The engine compiles each active rule into **exactly one anchor-denormalized
materialized view** — `advice_rule_{slug}` — carrying an `anchor_id` output column, read
at render time with `WHERE anchor_id = ?`, ordered by column ordinal (Spike-2's DDL
rules: matview `ORDER BY` needs ordinals; anti-join is `LEFT JOIN … IS NULL`).

Synthesis goes through `matview_manager`'s `reconcile_named_view` — the **named + diffed
+ torn-down** path (unchanged view skipped; changed view recreated; view dropped on rule
deletion). It **must not** use the content-addressed `watch_view_{hash}` path: content
addressing + per-anchor parameter inlining would mint one matview + one CDC subscription
**per anchor**, which is the documented **N-matview cliff** (memory: 1–2s/action at vault
scale; N tasks → N matviews). One matview per *rule*, denormalized over all anchors, is
the whole point — never one per anchor.

### Renderer owns the weave

The renderer reads `advice_rule_{slug}` filtered to the current anchor and weaves the
rows in under each matching anchor (ADR 0021 read-only-v1 gate). Mechanically this is the
**third suffix source** on `AppendedRowsProvider` (alongside the existing two). Advice
rows are marked **non-anchor** via `RowOrigin` (Increment A) so a rule's own output can
never satisfy another rule's `AnchorSelector` — **no recursion, no advice-of-advice** by
construction.

> **Amendment (2026-07-07, Increment F step 6):** v1 weaves advice **expanded by
> default, as direct placed children** — there is **no collapsible section** yet.
> Collapsed-by-default with **rerank-on-expand** activates only with **Increment H**:
> collapse exists to *bound rerank cost*, and there is no reranker in v1, so a collapse
> affordance would gate nothing. The `(anchor, rule)`-keyed toggle/collapse state
> described above is therefore an Increment-H concern, not v1.

### Rule blocks render their own status — fail loud, visibly

A rule block renders its compilation/runtime status inline: **active / compile error /
over-cap**. Synchronous parse errors render in place (the `live_query`-error surface);
**asynchronous** DDL failures (a rule that parses but whose matview synthesis fails at
reconcile time) need this same surface so a broken rule is visible, never a silent no-op.
This upholds the fail-loud-visibly priority order (works > visibly-degraded > clear-error
> never silent).

## Ratified sub-decisions (Martin, 2026-07-07)

1. **Edit policy = explicit `ACTIVE: t` flag.** Editing an *inactive* rule is free (no
   DDL churn). Flipping `ACTIVE` on triggers the single `reconcile_named_view` DDL
   reconcile. There is **no live-on-edit debounce** — the author controls exactly when
   the (relatively expensive) matview synthesis fires by toggling the flag.

2. **Anchor-cardinality cap = truncate + disclosed banner.** When a rule matches more
   anchors than the cap, the engine **truncates and discloses**: a banner on the rule
   block ("truncated at N anchors") **and** on each affected section. The design keeps a
   future per-section **`Expand`** affordance in mind — a read-time over-fetch beyond the
   cap for one section on demand. Synced rules **degrade visibly, never silently differ**
   between devices.

3. **`ScoringTemplate` enum primary + a RESERVED escape hatch.** *(Revised 2026-07-07:
   the initial "enum permanent" ratification was withdrawn — it had been given without
   the full analysis in view.)* The closed `ScoringTemplate` enum is the primary
   contract and the **only implemented path**: new relevance ideas ship as new enum
   variants (data-compatible additions). BUT the rule schema **reserves, from day one, a
   versioned raw-query field plus its refusal contract** (table-ref allowlist, no
   matview-refs, no IVM-unsupported constructs — the wrong answer at this boundary is a
   DB **hang**, not an error), so raw PRQL/SQL scoring can open up later **without a
   synced-rule-format migration**. The reserved field is *designed* now (schema +
   refusal-contract versioning) and *implemented* only when first needed; until then the
   v1 parser **refuses** any rule that uses it with an explicit "reserved, not yet
   supported" error — fail loud, never silent-ignore.

   > **Pointer (ADR 0023):** `ScoringTemplate` is the *retrieval* contract
   > (recall-oriented, top-N candidates); the **final ordering** may be refined by an
   > app-layer async **reranker** — see
   > [ADR 0023](0023-two-stage-relevance-app-layer-reranker.md). Retrieval reads
   > `LIMIT N`; the reranker picks the final top-K.

## Rejected alternatives

- **(A-raw) Unconstrained raw SQL/PRQL as a rule property (in v1).** User query text
  reaching matview DDL can **hang the DBSP graph** (the chained-matview /
  matview-on-matview hang class), and the content-addressed view path would **churn a
  fresh matview per keystroke** while the rule is edited. Unvetted user text at the DDL
  boundary is exactly the hazard the typed `AnchorSelector` + closed `ScoringTemplate`
  remove. The non-goal is **implementing** raw queries in v1, not raw queries ever:
  sub-decision 3's reserved, versioned raw-query field + refusal contract is the
  designed path to opening this up later, safely.

- **(B-pure) Closed templates as the USER surface** (rules are Rust enums only, edited by
  changing code). Runtime-hostile — contradicts the profile precedent (which proves
  user-editable config injecting queries is the established pattern) and puts every new
  rule on a compile-and-ship path, a dumping-ground trajectory. The closed enums are
  right **only as the compilation target**, not as the surface the user touches.

- **(C-naive) A `WEAVE_ON` flag on ordinary `live_query` blocks.** Reintroduces the
  **N-matview cliff** (each anchored live_query inlines its anchor parameter →
  content-addressed view per anchor) and **conflates a placed view with a floating rule**
  — the same query would render twice (once where placed, once woven). Placement and
  rule-authorship are different concepts and must stay separate blocks.

- **(D) Rhai pointcuts** (advice as a scripted aspect over the render tree). **IVM-opaque:
  a Rhai predicate cannot lower to SQL**, forcing a full scan per commit instead of
  incremental maintenance — the exact opposite of the anchor-denormalized matview. Plus
  the Rhai injection seam already carries a documented injection-safety history. Rejected
  on both performance and trust-boundary grounds.

## v1 cut

The single **lessons-for-tasks** rule ships as a **bundled-but-user-editable** rule block
(`source_language = 'holon_advice_rule_yaml'`, `ScoringTemplate::TagOverlapRecency`). It
is seeded by the vault but is an ordinary editable rule — **runtime-definable day one**,
with **no rules engine** and no second rule required to prove the mechanism.

> **Amendment (2026-07-07, Increment F step 6):** the bundled rule ships **INACTIVE**
> (`active: false` in `assets/default/index.org`). Activation is a **single user edit**
> (flip `active: true`), which fires the one `reconcile_named_view` DDL. Shipping it off
> by default (a) keeps the keystone's **≤1-active-rule** narrowing valid, and (b) avoids
> synthesizing a surprise matview (and paying its maintenance cost) on first boot for a
> feature the user has not opted into.

## Prerequisite (parallel change)

`matview_manager.rs:504` currently swallows a parse failure on **this exact boundary**:

```
let requires = parse_sql(&sql_for_view)
    .map(|stmts| extract_table_refs(&stmts))
    .unwrap_or_default();   // ← silent-swallow: unparseable DDL → empty deps
```

A synthesized rule whose SQL fails to parse would silently produce **no table
dependencies** (and thus a mis-scheduled / broken matview) with no error. This
`.unwrap_or_default()` is being fixed to fail loud as a **prerequisite / parallel
change** to Increment F — the rule-status surface above depends on this boundary
reporting failure rather than defaulting.

## Consequences

- Advice is **user-authored and runtime-definable** from v1: a rule is a vault block,
  discovered by the profile-pattern scan, synced through org/Loro.
- The engine synthesizes **one anchor-denormalized matview per rule** via
  `reconcile_named_view` (named/diffed/torn-down), never the content-addressed
  per-anchor path — the N-matview cliff is designed out, not merely avoided.
- `ScoringTemplate` is the **primary closed contract**; relevance evolves by adding
  variants. Raw user SQL/PRQL scoring is a **reserved, versioned escape hatch** —
  schema + refusal contract designed now, unimplemented (v1 parse refuses it loudly);
  opening it later requires no synced-format migration.
- Edit ergonomics are explicit: `ACTIVE: t` gates DDL reconcile; over-cap truncates with
  a disclosed banner (with a future per-section `Expand`).
- Broken rules **fail loud in place** (parse errors and async DDL failures both surface
  on the rule block), backed by the `matview_manager.rs:504` swallow fix.
- Increment F is re-scoped around this design (see the implementation plan); the plan's
  "one matview + CDC per task" scaling note is reframed as one matview per *rule*,
  denormalized over anchors.
- `ScoringTemplate` is the **retrieval** contract only; final relevance ordering is a
  second, app-layer stage — see [ADR 0023](0023-two-stage-relevance-app-layer-reranker.md)
  (two-stage relevance: matview retrieval `LIMIT N` + async reranker → top-K).
