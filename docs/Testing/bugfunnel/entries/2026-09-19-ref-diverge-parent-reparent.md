---
id: 2026-09-19-ref-diverge-parent-reparent
date: 2026-09-19
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The SUT re-homes a block to a different parent than the reference does, on
  `org` + `matview` + `block_raw` at once, and the land gate classified it
  pass-with-note for weeks because `org-blocks-ref-diverge`'s Match pattern
  could not tell a field delta from a set-membership excess.
---

## Bug

Found by a registry audit, not by a test run: the `known-red-narrow` lane was
asked to tighten `org-blocks-ref-diverge` after a verifier flagged its Match
pattern as overbroad (2026-09-18). Decoding what the pattern had actually been
absorbing turned up 79 panic lines in the wave-14/15b land-gate corpus that
carry a real FIELD delta, 19 of them this shape. 3-LOG: 72 and 19 — this
shape's 19 all sit in one log that is in both sets, so its count is the same
either way.

The keystone invariant fires correctly. What escaped is the classification: the
land gate printed these as `WARN known-red [org-blocks-ref-diverge]` and exited
0, so nobody ever triaged them.

Verbatim, `/tmp/holon-land-w14-1789580360/land-w14-keystone-full-1789644096.log:1014`:

```
[inv-blocks-match-ref/org] fields diverge from reference
  inv-blocks-match-ref/org: 21 blocks, reference: 21 blocks
  only in inv-blocks-match-ref/org (0): []
  only in reference (0): []
  field deltas (1):
    block:6a3532b5-bef3-45c7-abd0-b4902a677115: parent_id: sut=EntityUri("block:368857d2-b6f5-4a8d-5c03-a6daedd2100f") ref=EntityUri("block:d89347be-5498-4a82-bf6f-5591fbdb3180")
```

Equal block counts, empty membership diffs both ways, one `parent_id` delta.
All 19 lines co-fire on exactly three arms — `inv-blocks-match-ref/org`,
`inv-blocks-match-ref/matview` and `inv-block-parent/block_raw` — so the
disagreement is in the STORE, not in the org projection. Loro is not in the
arm set.

All 19 name the SAME sut parent `block:368857d2-…` against 19 different ref
parents, and one of those ref parents is `block:bulk-0-1`.

## Log set behind every count in this entry

All counts below were computed over these TEN wave-14/15b land-gate logs, all
under `/tmp/holon-land-w14-1789580360/`:

```
land-w14-keystone-full-1789644096.log      triage/runs/A1-int-nonwindowed.log
land-w14-nextest-1789584904.log            triage/runs/A2-lib-and-composed.log
land-w14-nextest-1789593946.log            triage/runs/B1-int-tests.log
land-w14-nextest-1789634032.log
land-w14-nextest-1789644096.log
land-w15b-nextest-1789664525.log
land-w15b-nextest-1789672138.log
```

The lane report also quotes a smaller set: `land-w14-keystone-full-1789644096`,
`land-w15b-nextest-1789664525` and `triage/runs/A2-lib-and-composed` are the
three logs classified end to end through `scripts/keystone-known-reds.sh`.
Where a count differs between the two sets it is given for both and the
smaller one is marked 3-LOG. Over those three logs the population the narrowed
`org-blocks-ref-diverge` released is 72, not 79; the other seven logs
contribute one line each.

## Root cause

Two separate causes, and only the second is addressed here.

**The masking (this entry's gap).** `docs/Testing/KeystoneKnownReds.md`'s
`org-blocks-ref-diverge` row matched on
`diverged from the oracle: .*"inv-blocks-match-ref/[a-z_]+".*fields diverge from reference`.
That string is the emitter's HEADLINE
(`crates/holon-pbt-core/src/block_compare.rs:159`) and is printed for every
divergence the invariant can report, so the pattern classified the entire
invariant as one known red. The row's decoded instance is a set-membership
EXCESS with zero field deltas; a `parent_id` delta is a different defect
entirely and should always have been novel. The registry is the land gate's
oracle over failure signatures, and this one could not express the distinction
the diff body already prints.

**The underlying divergence (NOT root-caused, NOT fixed).** This is
`org-blocks-ref-diverge` cause C, first decoded 2026-08-11 and still open. The
SUT parents what looks like a split product to one block while the reference
parents it to another. Because the delta reaches `block_raw`, if the SUT is the
one in error then a block is being re-homed in the STORE, which is data
corruption rather than a projection wrinkle. **Which parent is correct is an
open question** and must be settled from split semantics before either side is
changed to agree with the other. The `block:bulk-0-1` ref parent points at the
same `rematerialize_file_ingested` area as `bulk-add-sibling-order`.

Frequency is NOT established: all 19 lines come from a single log, so they are
shrink-tail re-panics of one case, not 19 independent draws.

## Missing piece

A Match pattern that reads the structured diff body instead of only its
headline. `render_block_diff` has printed `only in …` / `field deltas (N):`
since it replaced the whole-snapshot dump, so the information needed to
separate these families was in every log the whole time — the registry just
never looked at it.

Compounding it: the 2026-07-31 fixture corpus predates `render_block_diff`, so
the pattern-drift guard could not have caught the overbreadth either. Its 123
archived panics carry the old dump wording and contain no `field deltas` text
at all.

## Remedy

PARTIAL — classification fixed, defect open.

- `org-blocks-ref-diverge`'s pattern now requires `field deltas \(0\):`, so it
  matches only the set-membership shape it was written for.
- This shape is registered on its own as `ref-diverge-parent-reparent`
  (`known-red`, UNOWNED) with the decoded evidence above, placed after
  `editor-text-mirror` so co-fired lines keep their existing attribution.
- The pattern was TIGHTENED after adversarial verification: it first read
  `field deltas \([1-9][0-9]*\):.*parent_id`, which contradicted the "exactly
  one delta" claim above by also claiming two-field and twelve-delta sets. It
  then anchored on `field deltas \(1\):` followed immediately by the single
  delta line. That excluded multi-BLOCK sets but not multi-field deltas on one
  block; see the next point.
- **Multi-field deltas on ONE block needed a second fix, and the first claim here
  was false.** `field deltas (N)` counts divergent BLOCKS, not fields:
  `render_block_diff` pushes one entry per block (`block_compare.rs:243`) and
  `field_deltas` joins that block's fields with `"; "` (`:302`), so a block
  differing in two fields still prints `field deltas (1):`. Anchoring on the count
  therefore excluded multi-BLOCK sets only, and every pattern still matched
  whenever its field came first in `field_deltas`' fixed emission order —
  `parent_id` is first of all. All three patterns now end in `[^;]*\\n`, requiring
  the matched block's delta line to reach its newline with no `; ` after the field
  segment. Verified through `scripts/keystone-known-reds.sh` on single-block
  synthetics: `parent_id`+`content`, `parent_id`+`marks`, `content`+`marks`,
  `content`-ref-empty+`marks` and `tags`+`content` all classify NOVEL, while the
  single-field control for each row still matches. No real line moved: zero lines
  in the ten-log set carry a `; ` inside a delta.
- A current-format fixture corpus
  (`crates/holon-integration-tests/hand-authored-regressions/fixture-logs-2026-09-19/`)
  now pins this row at x19. Reverting the narrowing collapses the three
  `ref-diverge-*` rows back into `org-blocks-ref-diverge` x29 and the fixture
  reds — verified by inversion, with the registry restored byte-for-byte
  (sha256 `0cd3b868…`).

**RATIFICATION OWED.** The registry's own rule is that a novel signature is
triaged first and registered only once Martin ratifies it as a known red. This
row is triaged but not ratified.

Still open: the divergence itself, and its true frequency. The next step is a
deterministic repro from the transition telemetry in
`land-w14-keystone-full-1789644096.log` (the same `[inv-sql-budget] <Transition>:`
counting method that gave `bulk-add-sibling-order` its 3-transition case), then
the split-semantics ruling on which parent is right.
