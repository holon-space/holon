# H7 — Ordering-authority reconciliation (deferred validation for Phase 7)

**Date:** 2026-05-29
**Question (from senior review, plan H7):** does the proposed fixed priority
`Loro > Org > Markdown > Turso` (ADR 0007 weakness #3, plan decision #4)
reproduce *today's* ordering behavior in each blessed manifest? The hypothesis
flagged a risk: `BlockOrdering` + `org_sync_controller` "treat org line order as
the authoritative total order," which looks like it contradicts `Loro > Org`.

**Verdict: ✅ behavior-preserving for all four blessed manifests.** The apparent
contradiction dissolves — org line order is authoritative *only when Loro is
absent*, which is exactly what `Loro > Org` encodes. Read-only analysis; no code
changed.

## How authority is selected today

Authority is a **binary switch on Loro presence**, not a four-way priority:

- `BlockOrdering::consolidator()` / `is_loro_backed()`
  (`holon-core/src/block_ordering.rs:131,141`) delegate to
  `caps.profile().has_loro()` (`sql_block_operations.rs:548-554`). Loro present →
  `Consolidator::Loro`; else `Consolidator::Sql`.
- The org reconciler branches on exactly this in its place-loop
  (`holon-orgmode/src/org_sync_controller.rs:887`):
  - **`Consolidator::Loro`** (lines 887–937): "Loro owns order." Org file changes
    are *adopted into* Loro — each text block is `place()`d after its file
    predecessor (org document order → Loro tree position). Loro stays the single
    authority; **org is an input channel, not a competing authority.**
  - **else / `Consolidator::Sql`** (lines 938–967): "the SQL store is the sole
    order owner, and the file's line order is the authoritative TOTAL order,"
    realized via `place_all` minting one gap-free key sequence per parent in
    document order. The Turso `sort_key` column is the **projection sink** here,
    never the authority — `ORDER BY sort_key` just reproduces the file.

So the Turso column is *always* a downstream sink, never an order owner — which
is why it correctly sits at the **bottom** of the fixed priority.

## Per-manifest reconciliation

| Blessed manifest | Adapters | Today's authority | Fixed-priority winner | Match |
|---|---|---|---|---|
| Full | Loro, Org, Markdown, Turso | Loro (`Consolidator::Loro`; org adopted via `place`) | Loro | ✅ |
| sql_only | Turso, Org | Org line order (`place_all`→`sort_key`; Turso = sink) | Org (> Turso) | ✅ |
| loro_backend | Loro | Loro | Loro | ✅ |
| org_create_ordering | Org | Org line order | Org | ✅ |

Matches project memory ("children read Loro authority" for Full;
`inv-live-children-match-ref` is the SqlOnly org-line-order oracle).

## Caveats / unexercised edges

1. **`Org > Markdown` is currently unexercised.** No blessed manifest contains
   `{Org, Markdown}` *without* Loro, so the relative priority of two
   line-ordered file adapters is never pitted in tests today. It is *consistent*
   (Markdown would behave like Org — line order — when it is the sole non-Loro
   source) but **unvalidated**. If Phase 7 ever blesses an Org+Markdown-no-Loro
   manifest, add an invariant that exercises the tie-break.
2. **Authority is binary today, priority is 4-way.** The fixed priority is a
   faithful *generalization* of the current binary switch, not a literal match
   to existing code. Phase 7's `Wiring::ordering_authority()` should compute
   "highest-priority wired storage adapter" and assert it equals
   `Consolidator::Loro ⇔ Loro wired` for the blessed set (a cheap regression
   guard).
3. **Markdown has no sync-controller place-loop analog yet.** `holon-markdown/`
   has parser/renderer but no order-adoption loop equivalent to
   `org_sync_controller`'s lines 887–967. Wiring Markdown as an order *owner*
   (vs. a pure renderer) is net-new work, not covered by this reconciliation.

## Recommendation for Phase 7

Commit the fixed priority `Loro > Org > Markdown > Turso` as
`Wiring::ordering_authority()`. Add a unit assertion over the 4 blessed
manifests that `ordering_authority()` agrees with today's
`consolidator()`-derived owner (Loro-wired ⇒ Loro, else top-most file adapter).
Leave `Org > Markdown` documented-but-unexercised until a manifest needs it.
*Falsifies the "fixed priority is behavior-preserving" risk: it holds for every
blessed manifest; the only gap is an untested adapter pairing, not a conflict.*
