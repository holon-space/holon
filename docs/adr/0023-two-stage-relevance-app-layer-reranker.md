# ADR 0023: Two-stage advice relevance — incremental retrieval + app-layer async reranker

**Status:** Accepted (2026-07-07). Decided via a hostile Fable review of the
single-stage relevance design; the sub-decisions below were ratified by Martin the
same day (see "User-ratified sub-decisions"). This ADR builds directly on
[ADR 0022](0022-runtime-definable-advice-rules.md): it does **not** change how the
advice rule or its matview are defined — it splits *relevance* into a retrieval stage
(ADR 0022's matview, unchanged) and a new app-layer reranking stage.
**Deciders:** Martin (+ adversarial Fable review)
**Relates to:** [ADR 0015](0015-computed-placement-and-curated-state-primitives.md)
(entity- vs element-identity; the shared `Cell` seam a reranked cell writes into),
[ADR 0016](0016-occurrence-keyed-focus-authority.md) (occurrence-keyed identity),
[ADR 0019](0019-capmap-dependency-injection.md) (CapMap = the PBT-composition DI
container — why the production `Reranker` seam is **not** CapMap),
[ADR 0021](0021-advice-suppression-storage-and-readonly-v1.md) (suppression storage +
read-only v1; the disclosed-degradation surface this ADR reuses),
[ADR 0022](0022-runtime-definable-advice-rules.md) (`ScoringTemplate`, `BoundedK`,
`reconcile_named_view`; retrieval contract), `docs/Proposals/advice-feature-implementation-plan.md`
(Increment F, Spike-2).

## Context

Under ADR 0022, advice relevance is a single stage: the engine compiles each rule's
`ScoringTemplate` into one anchor-denormalized materialized view, and the renderer reads
the top-K rows for an anchor with `WHERE anchor_id = ?`. That scoring is **symbolic and
IVM-lowerable by construction** (tag overlap + recency, computed incrementally in the
DBSP graph). It is cheap and always-fresh — but it is also *shallow*: it cannot read the
actual content of an anchor and a candidate together and judge "is this lesson genuinely
relevant to this task right now?" the way a context-aware model can.

The obvious temptation is to push a smarter scorer **into** the matview — a Turso UDF
that calls a model per row. That is exactly the wrong place (see Rejected alternatives):
it puts model inference in the commit path (the measured latency dominator — memory:
1–2s/action at vault scale is already CDC/consolidator-bound), it defeats batching
(scalar UDFs are row-at-a-time), and DBSP would re-invoke the UDF on unrelated deltas,
multiplying uncontrolled API calls. And a matview whose contents depend on a model
version is **stale-by-construction** — the cache-invalidation problem lives inside the
IVM graph where we cannot key it on the model.

The retrieval stage is right where it is. The relevance *judgment* belongs one layer out,
in the app, where it can be batched, cached, made async, and gated on user attention.

## Decision — advice relevance is TWO-STAGE

**Stage 1 — retrieval (IVM matview, ADR 0022, unchanged).** The per-rule
anchor-denormalized matview stands exactly as ADR 0022 specifies. It was already
un-capped per anchor (the whole point of anchor-denormalization). The only change is at
the *read*: retrieval reads **`LIMIT N`** candidates for an anchor rather than the final
`LIMIT K`. `ScoringTemplate` is henceforth the **retrieval contract** — recall-oriented,
producing the top-N candidate set (N ~20–50), not the final display order.

A new **`BoundedN`** newtype mirrors `BoundedK` (ADR 0022): the retrieval width. Parse
**refuses `K > N`** — you cannot ask to display more rows than were retrieved. `BoundedK`
remains the final display cap; `BoundedN` is the candidate-set width feeding the
reranker.

**Stage 2 — rerank (app layer, post-query, async).** After retrieval returns N
candidates for an anchor, the app batches the N `(anchor, candidate)` pairs through a
context-aware model, and the final **top-K** is chosen from the rerank scores. This
happens **in the app layer, never in the DB**, always **asynchronously** (never on the
commit or render path).

**Reranker-as-Turso-UDF is REJECTED** for three reasons, any one sufficient:
1. **Model inference in the commit path.** The commit/CDC/consolidator path is the
   measured latency dominator; a per-commit model call makes every keystroke pay
   inference latency.
2. **Row-at-a-time scalar UDFs kill batching.** A reranker's whole efficiency is scoring
   N pairs in one call; a scalar UDF is invoked per row, one call each.
3. **DBSP incremental recompute re-invokes UDFs on unrelated deltas** — an uncontrolled
   multiplication of (paid, rate-limited) API calls triggered by edits that have nothing
   to do with the anchor.
Plus: **model-version-dependent matview contents are stale-by-construction** — the cache
key must include the model + rubric version, which cannot live inside the IVM graph.

### Model seam

`Reranker` is a domain trait:

```
trait Reranker {
    // absolute rubric-anchored scores for N candidates against one anchor, one batch
    fn score_batch(&self, anchor: &AnchorContent, candidates: &[CandidateContent])
        -> Result<Vec<Score>>;
}
```

- **Production wiring = fluxdi** (the Clock-seam pattern), **not CapMap.**
  [ADR 0019](0019-capmap-dependency-injection.md) draws CapMap as the
  **PBT-composition** container; a real external model client is a production runtime
  dependency, injected the way the `Clock` seam is. In the **keystone PBT** the reranker
  is a **CapMap capability** carrying a **deterministic fake** (e.g. `score =
  hash(anchor_id, lesson_id)`), so the reference model predicts the exact final order.
- **First implementation = ONE generic OpenAI-compatible HTTP client** parameterized by
  `base_url` (user decision: it opens the most doors — most local and hosted inference
  servers speak the OpenAI wire format). Local ONNX/`candle` or Claude-over-MCP can land
  **later behind the same `Reranker` trait**; there is **no per-provider Rust**. This
  honors the *spirit* of the MCP-declarative-YAML directive — that directive is
  **MCP-scoped** (MCP clients are declarative YAML sidecars, never client-specific Rust),
  and a reranker HTTP client is not an MCP client, so the directive is **not violated
  here**.
- **Async discipline:** the reranker runs as a **spawned task that writes a signal cell
  only** — never `block_on` in a render or commit path (memory:
  turso-storage-pbt deadlock is the standing evidence for why blocking on an external
  call inside these paths deadlocks). Completion writes the reordered cell; the render
  path merely observes it.

## User-ratified sub-decisions (Martin, 2026-07-07)

1. **Rerank-on-expand.** Advice sections are **collapsed by default** (header + count
   visible). **Expanding** a section triggers the rerank; the content appears
   **already-ranked**. There is **no visible reordering** (the user never watches rows
   shuffle), and API cost is **bounded by user attention** — sections never expanded
   never cost a call. Render-then-reorder was explicitly **not** chosen.

   > **Amendment (2026-07-07, Increment F step 6):** collapsed-by-default +
   > rerank-on-expand is an **Increment H** behavior and presupposes a reranker. In
   > **v1 there is no reranker**, so advice weaves **expanded by default, as direct
   > placed children** (no collapsible section) — see ADR 0022's matching amendment.
   > The collapse affordance exists to **bound rerank cost**; with no rerank to bound,
   > it lands together with the reranker in Increment H, not before.

2. **`rerank:` field RESERVED NOW** in the rule schema, following ADR 0022's `raw_query`
   Reserved pattern exactly: an **always-failing deserializer** ("reserved, not yet
   supported"), so a synced rule that uses `rerank:` on an **old binary errors loudly**
   rather than half-parsing. When implemented, **`rerank:` names a MODEL PROFILE only** —
   it carries no endpoint, no key, no consent. The **endpoint + API key + the "vault
   content may leave this machine" consent** live in **device-local preferences**
   (`PrefType::Secret` + env override — the `todoist.api_key` precedent,
   `crates/holon-frontend/src/preferences.rs:153-160`), **never in vault blocks or synced
   rules.** This is a hard boundary: enabling rerank on device A must **not** silently
   exfiltrate vault content from device B. An unconfigured profile / offline / timeout /
   API failure → the section renders **retrieval order** with a per-section
   **"unranked" badge** — the ADR 0021 disclosed-degradation surface, reused.

3. **Score contract = listwise-batched with rubric-anchored ABSOLUTE scores + pointwise
   incremental** (a user-designed hybrid).
   - **Initial fill** for an anchor scores **all N pairs in ONE batched call** whose
     prompt defines an **absolute 0–100 relevance rubric**. This gets listwise quality
     (the model sees the alternatives) *while* each score is **absolute-against-the-rubric**
     → approximately **pointwise-comparable and cacheable PER-PAIR** (unlike a pure
     listwise ranking, which is only meaningful relative to the exact set scored).
   - **Incremental updates** (a new candidate arrives; a dismissal backfills a slot) score
     **only the new pairs**, including a few already-cached scores in the prompt as
     **calibration references** (few-shot anchoring keeps the scale aligned across calls).
   - **Residual cross-call calibration drift is accepted** and **self-heals on the next
     full listwise refresh** (triggered by a cold cache, or set churn past a threshold).
   - **Cache key:** `(anchor_id, lesson_id, model_id, rubric_version,
     prompt_content_hash)`, where `prompt_content_hash` covers the **exact serialized
     prompt inputs** (anchor content + candidate content + included tags/props). Staleness
     is impossible by construction: any change to the inputs changes the hash → a miss.

## Cache

The rerank scores live in a **plain device-local Turso table** `advice_rerank_scores` —
**NOT a matview, no CDC, never synced** (precedent: the device-local
`navigation_history` / `sync_states` tables). Consequences of that choice:

- A **dropped cache = re-paid API cost** (acceptable — it is a pure cost, never a
  correctness issue).
- **Staleness is impossible by construction** via the content-hash key (see sub-decision
  3): a stale row can never be read as fresh, because any input change mints a new key.
- **All N pairs are cached on the initial fill** → a later **dismissal backfill is a pure
  cache hit, zero API calls** (the backfilled candidate was already scored in the initial
  batch).

## Keystone PBT

Determinism ends **exactly at the model boundary**. With the deterministic fake
(sub-decision-3 hash), the keystone asserts:

- **Final K = fake-ordered top-K of the retrieval-N** (the reference model predicts the
  exact final order).
- **Suppression anti-join holds THROUGH rerank** — a dismissed `(anchor, lesson)` never
  surfaces, before or after reordering (ADR 0021 / Spike-2 anti-join, now downstream of a
  second stage).
- **Failure path** renders **retrieval order + a degraded badge** (unconfigured / offline
  / timeout / API failure — sub-decision 2).
- **The dismiss-during-in-flight-rerank race**, as a **generated transition
  interleaving**: an async rerank completion **must re-check suppression before writing
  the cell**, or a row dismissed while the rerank was in flight would **resurrect for a
  frame**. This is the one genuinely concurrent hazard the two-stage split introduces, so
  the keystone generates the interleaving rather than trusting a happy-path ordering.

## Ordering vs the embedder / vector track

**The reranker ships FIRST; the embedder comes later; they are explicitly NOT gated on
each other.** They are different stages:

- The **reranker** refines the *ordering* of an already-retrieved candidate set (stage 2).
- The **embedder** is a **retrieval-stage** design of its own (stage 1): vector similarity
  is **not IVM-lowerable** → it **cannot** be a `ScoringTemplate` variant; it needs its
  own embedding storage, staleness/`model_version` reindex, and kNN read design. When it
  lands it will **REUSE this ADR's seam + cache** (the reranker sits downstream of
  whatever retrieval produced the N).

**HONEST CAVEAT:** until the embedder lands, **rerank quality is capped by tag-overlap
recall.** A lesson that shares **no tag** with the anchor is **unreachable by retrieval**,
so no reranker — however good — can surface it. The reranker improves the *ordering of
what tag-overlap already retrieved*; it does not widen recall. Widening recall is the
embedder's job.

## v1 cut

- **Ships with Increment F (parse-level only):** reserve the **`rerank:` field** +
  **`BoundedN`** newtype + the **`K ≤ N` refusal** in the rule schema. That is all that
  lands in F — the schema is forward-compatible and the reserved field refuses loudly.
- **The reranker implementation is its own increment AFTER F's read-only weave lands:**
  the `Reranker` trait, the OpenAI-compatible client, the `advice_rerank_scores` cache,
  the expand-trigger, the "unranked" badges, and the keystone fake + race transition.
  Sequencing it after F keeps F a pure read-only symbolic-relevance weave, with the
  model-dependent, async, cost-bearing machinery isolated in a later increment.

## Consequences

- Advice relevance is **two-stage**: ADR 0022's matview is the **retrieval** contract
  (top-N, recall); the app-layer reranker owns **final ordering** (top-K). `ScoringTemplate`
  is reframed as retrieval-only.
- **No model inference ever enters the DB / commit / render path** — the reranker is
  app-layer, spawned, signal-cell-only, and triggered by **expand** (attention-bounded
  cost).
- The production seam is **fluxdi** (`Reranker` trait), **not CapMap** (CapMap stays the
  PBT-composition container per ADR 0019); the keystone drives a **deterministic fake**
  through a CapMap capability.
- **One generic OpenAI-compatible client** (base_url-parameterized) is the first backing;
  local/Claude-over-MCP land later behind the same trait — **no per-provider Rust**, and
  the MCP-declarative-YAML directive (MCP-scoped) is not violated.
- **Consent + credentials are device-local** (`PrefType::Secret`), never synced; a synced
  rule's `rerank:` names a **profile only** — no cross-device exfiltration.
- The **cache** is a device-local, un-synced, non-matview table; staleness is impossible
  by construction (content-hash key), and dismissal backfill is free.
- The **dismiss-during-rerank race** is a real new hazard, pinned by a generated keystone
  interleaving (re-check suppression before the async cell write).
- **Reranker before embedder**, ungated; until the embedder lands, quality is **capped by
  tag-overlap recall** (disjoint-tag lessons are unreachable) — stated as an honest,
  temporary limitation, not a silent one.
