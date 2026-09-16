---
id: 2026-09-16-stale-cross-doc-copy-survives-in-the-aggregator-file
date: 2026-09-16
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Write-back prunes the stale cross-doc block from ownership but leaves its copy
  in the aggregator file on disk, so `Overview.org` keeps a flat duplicate of a
  block another file owns.
---

## Bug

`holon-integration-tests::org_suite writeback_stale_cross_doc_prune::stale_cross_doc_block_is_pruned_not_adopted`
fails at `crates/holon-integration-tests/tests/org_suite/writeback_stale_cross_doc_prune.rs:155`:

```
    Overview.org still carries the stale bulk-0-0 copy — it was not pruned:
    <Overview.org body>
```

Found by the wave-14 land-gate triage
(`/tmp/holon-land-w14-1789580360/triage/TRIAGE.md` row 30), classified
PRE-EXISTING-ON-MAIN. Identical in both trees — A 3.0 s, B 3.7 s
(`runs/A1-int-nonwindowed.log`, `runs/B1-int-tests.log:361`) — against main
`be08291ccf1b`.

**The asserts BEFORE this one pass, and that is the finding.** The test injects
an on-disk phantom: `Overview.org` gains a flat copy of `bulk-0-0`, which is
authoritatively owned by the day page. It then settles writeback and checks the
three things a correct prune must do:

- `:141` `count_rows(bulk-0-0) == 1` — passes, the block exists once.
- `:146` `parent_of(bulk-0-0) == DAY_PAGE` — passes, `bulk-0-0` was NOT adopted
  away from its authoritative day page.
- `:155` `!overview_disk.contains("bulk-0-0")` — FAILS.

So the store-level half of the remedy works: the phantom did not steal
ownership. What does not happen is the disk half — the aggregator file keeps a
copy of a block that another file owns, and only a re-render or a later
write-back would remove it.

## Root cause

UNATTRIBUTED within the write-back leg. The observable is precise: ownership is
correct in the store and the stale text survives on disk in the aggregator. That
places the gap after the store decision and before (or inside) the file
projection for the non-owning document — either the aggregator's file is not
re-rendered on this path, or it is re-rendered from a source that still contains
the phantom.

Not chased further here: this is a docs/triage lane, and the mechanism needs the
write-back leg read with the test's settled timeline in hand.

## Missing piece

The composed keystone does not seed this shape. `Overview.org` / `overview_org`
appears nowhere under `crates/holon-integration-tests/src/pbt/`, and the nearest
transition, `StaleExternalRewrite`, replays a doc's CURRENT content (a stale
rewrite of the same document) rather than planting a stale cross-document
aggregator copy. So no generated case reaches "another file carries a flat copy
of a block this file owns, then writeback settles".

The ORACLE secondary: no composed invariant expresses "a file that does not own
a block does not carry it" — the org render fixed-point invariant compares a
document against its own store content, which a foreign-owned flat copy would
satisfy on both sides.

## Remedy

OPEN. The gap-closing work is a composed transition that plants a stale
cross-doc copy plus an invariant that judges the on-disk aggregator against the
ownership map — which is what would have kept this under the keystone instead of
in one org_suite binary.

The product fix needs the mechanism first. The test is already written to
discriminate the two halves (ownership vs disk), and it says the disk half is
the one that is broken, so the next step is to follow the write-back path for
the NON-owning document when its file already carries the text.
