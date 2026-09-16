---
id: 2026-09-16-logseq-db-import-lands-17-of-206-blocks
date: 2026-09-16
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A Logseq DB graph import through `BlockOrdering` reports 17 created blocks
  where the fixture expects 206; the assertion lives only in its own binary and
  the keystone has no Logseq-DB import path at all.
---

## Bug

`holon-integration-tests::logseq_db_import_store logseq_db_graph_imports_into_a_real_store`
fails at `crates/holon-integration-tests/tests/logseq_db_import_store.rs:201`:

```
    assertion `left == right` failed
      left: 17
     right: 206
```

Found by the wave-14 land-gate triage
(`/tmp/holon-land-w14-1789580360/triage/TRIAGE.md` row 29), classified
PRE-EXISTING-ON-MAIN. Both trees fail identically — A 1.584 s
(`runs/A1-int-nonwindowed.log:153`), B 1.958 s (`runs/B1-int-tests.log:263`) —
and B is main `be08291ccf1b`, so no lane caused it.

The test is the full import path against a real store: read the HolonTest datom
fixture, `project` the datoms into blocks, resolve `dyn BlockOrdering` from the
injector, `enter_store` through it, then assert
`report.blocks_created == EXPECTED_BLOCKS` (`:= 206`, declared at `:59`) and, at
`:208`, `report.blocks_persisted_by_authority == EXPECTED_BLOCKS`. It stops at
the first, so 17 blocks were created where 206 were expected — roughly one in
twelve.

The test's own comment explains why the assertion is at the call site rather
than trusted from the report: "A decline is not a failure at the call site, so
it has to be asserted here: an authority that routes the creates elsewhere
cannot own their order either, and the import would look successful anyway."
That reasoning is what makes this worth recording rather than dismissing — the
import DID look successful, and only this count says otherwise.

## Root cause

UNATTRIBUTED, and the two readings are not separable from this capture:

1. the projection or the store-entry path silently declines ~92% of the blocks
   (the defect the test was written to catch), or
2. `EXPECTED_BLOCKS = 206` is stale against the fixture — the datom fixture
   changed and the hand-written constant did not.

Reading 1 is the more serious and cannot be excluded: nothing in the log shows
which 17 blocks landed, and a partial import that the report calls successful is
exactly the failure mode the assertion exists for. Reading 2 is easier to check
and would be the first thing to measure — run `read_datoms`/`project` and count
the projection before it reaches `BlockOrdering`.

## Missing piece

The composed keystone has no Logseq-DB import path. `logseq` appears under
`crates/holon-integration-tests/src/pbt/` only in `driver_input.rs`, and there is
no transition that imports a datom set, so the projection-plus-store-entry path
is exercised by exactly one side binary. Nothing else in the fleet would notice
a fractional import.

The ORACLE secondary: the count assertion is a constant in the test file, not an
invariant over the import's own input, so it cannot distinguish "the fixture
changed" from "the import dropped rows".

## Remedy

OPEN. The discriminator is cheap and belongs in the test rather than in a lane:
assert the count against the PROJECTION's own size (blocks the projector
produced) instead of a literal, and keep a separate assertion that the
projection is non-trivial. Then a fixture change moves the expectation with it
and a dropped-row defect stays red.

Whoever picks this up should treat reading 1 as live until reading 2 is
disproved — a store-entry path that declines 189 of 206 blocks without failing
is a data-loss-shaped defect, and CLAUDE.md's error philosophy puts that above
all other priorities.
