---
id: 2026-09-03-pinblock-misses-block-behind-lazy-day-page-shell
date: 2026-09-03
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Shift+click cannot pin a block that sits under a journals day page, because
  the main panel renders that day page as a lazy embedded shell with none of its
  children, so the block has no bullet to bind a pin intent to.
---

## Bug

The keystone's `PinBlock` transition fails deterministically:

```
[PinBlock] shift+click on block:bulk-1-2 did not pin it into the right sidebar —
focus_roots holds [{"region": String("main"), "root_id": String("block:journals")}].
The bullet bound no shift-intent and the click degraded to a bare focus;
block:bulk-1-2 is most likely not a descendant of focus…
  MISS REASON: block:bulk-1-2 renders NO node in region main — the panel is not
  showing this entity at all. It renders 4 distinct entities:
  [block:368857d2-…, block:__virtual:368857d2-…, block:default-main-panel,
   block:journals]
```

Assertion at `crates/holon-integration-tests/src/pbt/composed/components.rs:3297`
(`:3277` on main — same assertion, line shifted). Found by an A/B triage of a
keystone smoke red (`w-lowcode-inc1-smoke.55633.log:13019-13020`).

## Root cause

Not root-caused to a layer; the causal path is established:

- `BulkExternalAdd` inserts `bulk-1-0/1/2` as children of the auto-created
  journals day page `block:368857d2-…` (`PageId::for_path("Journals/2026-01-16")`).
- `NavigateBack(Main)` sets `focus_roots(main) = block:journals` — confirmed in
  the panic text.
- Under that focus root the main panel renders the day page as a **lazy embedded
  page shell**: the page node plus its virtual slot
  `block:__virtual:368857d2-…`, and none of its children. So `bulk-1-2` has no
  bullet, the shift-click binds no `focus_pin` intent, and the click degrades to
  bare focus.

Ruled out: the pair-inc1 projection withhold, the ingest metadata reconcile, and
the write-back membership settle change — the red predates all three (it fires on
main). The `[FileSyncController] write-back SKIPPED … membership` WARN at
`:13115` is not causal; the same WARN fires benignly in green hand-authored
cases at the tip.

A reduced probe (transitions 1+5 only) is red for a different and vacuous reason
— boot focus is `block:structural-page`. That pins `NavigateBack → journals` as
the step that makes the case non-vacuous, and confirms the miss is journals-feed
lazy embedding rather than a lost write.

**Rate table, verbatim from the verdict** (3 runs per revision, ~10s each;
15/15 red, byte-identical MISS REASON at every revision):

| rev | bookmark | result |
|---|---|---|
| 6b6e47c3595c | sw/lowcode-inc1 (tip) | RED 3/3 |
| e33bfa871948 | sw/ingest-contract | RED 3/3 |
| d596702637dd | sw/pair-inc1 | RED 3/3 |
| ff7448cc5bf0 | sw/org-writeback-reds | RED 3/3 |
| 89e2efeaa1ff | main | RED 3/3 |

## Missing piece

Two hypotheses, both live, **no ruling made**:

1. **Prod render drop.** The journals feed should render day-page children
   inline, and does not. This would make it a sibling of the hand-authored
   regression case `main-panel-drops-refocused-split-block`
   (`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`),
   a confirmed render drop whose layer is also unidentified. Same family as
   `inv-embedded-page-collapsed-lazy` / `inv-viewmodel-tree-virtual-slots`.
2. **Generator precondition gap.** Lazy embedding is the intended render, and
   `PinBlock`'s precondition must exclude a block sitting behind a virtual slot —
   the transition asserts an interaction the UI never offers.

The two remedies are opposite (fix the panel vs. narrow the generator), so the
distinction has to be settled before either is built.

## Remedy

Registered as `pinblock-lazy-day-page-shell` in
`docs/Testing/KeystoneKnownReds.md` (pass-with-note) so the red does not block a
land while the hypotheses are open.

A deterministic replay exists and is the reusable artifact:
`docs/Testing/fixture-logs-2026-09-03/pinblock-probe.jsonl`, case
`pinblock-bulk-1-2-probe`, authored verbatim from the log's minimal failing
input with the wiring pinned to the failing draw
(`storage={Turso} sync={} actors={}`). It runs through
`tests/hand_authored_regressions.rs` with `HOLON_HAND_AUTHORED_SIDECAR` +
`HOLON_HAND_AUTHORED_CASE`, and is promotable into
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl` as-is
once the hypothesis is ruled.
