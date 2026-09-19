---
id: 2026-09-19-ref-diverge-content-text-drift
date: 2026-09-19
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Block content drifts between the SUT and the reference with both sides
  non-empty; 44 of 45 observed lines are the block-side shadow of
  `editor-text-mirror`, and all 45 were absorbed as `org-blocks-ref-diverge`
  pass-with-note.
---

## Bug

Third of the three shapes uncovered by the registry audit behind
[[2026-09-19-ref-diverge-parent-reparent]] and
[[2026-09-19-ref-diverge-content-ref-empty]].

45 panic lines in the ten-log set named below; 3-LOG: also 45, since all of
them sit in two logs that are in both sets. The delta is a content field where
BOTH sides hold text:

```
land-w15b-nextest-1789664525.log:12831
    block:3c3f34d2-02eb-409f-bf5e-32087fb8515b: content: sut="f" ref="fv5"
land-w15b-nextest-1789664525.log:13156
    block:6023afed-cfb9-48ab-a1dd-44d5bc7c09c6: content: sut="2 f Y" ref="2f Y"
```

A truncation and a whitespace shift. Arm set is the same five as
`ref-diverge-content-ref-empty` — `inv-blocks-match-ref/{org,matview,block_raw}`
plus `inv-block-content/{block_raw,sql}`.

**44 of the 45 also carry `inv-editor-text/mirror` `Live editor text
mismatch` on the same block.** That is not a coincidence: it is the already-
registered `editor-text-mirror` family seen from the block side. The block's
stored content and the reference disagree on exactly the text the mirror is
reporting.

The ONE line that is not co-fired is a HYBRID, and it matters because it is the
only observation this row actually claims —
`triage/runs/A2-lib-and-composed.log:2611`:

```
[inv-blocks-match-ref/org] fields diverge from reference
  inv-blocks-match-ref/org: 9 blocks, reference: 8 blocks
  only in inv-blocks-match-ref/org (1): ["block:gen-11"]
  only in reference (0): []
  field deltas (1):
    block:c2: content: sut="c2" ref="c"
```

A cause-A set-membership EXCESS and a content field delta on the same arm at
once. Under the old pattern this classified as `org-blocks-ref-diverge` and the
content half was invisible.

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

**The masking (this entry's gap).** As in the two sibling entries: the registry
pattern anchored on the emitter headline
(`crates/holon-pbt-core/src/block_compare.rs:159`) rather than on the diff body
`render_block_diff` prints, so one row claimed every shape the invariant emits.

A second-order effect specific to this shape: because `org-blocks-ref-diverge`
sits EARLIER in the registry than `editor-text-mirror` and the classifier stops
at the first matching key, the 44 co-fired lines were being taken from
`editor-text-mirror` too. That family's hit counts have been understated for as
long as both rows have coexisted.

**The underlying divergence.** For the 44, believed to be `editor-text-mirror`
and owned there — no separate investigation is proposed. For the hybrid, NOT
root-caused: a block set excess and a content delta co-occurring suggests one
operation both added a block and wrote the wrong text, but a single observation
supports no mechanism claim.

## Missing piece

Same as the siblings: a Match pattern reading the structured diff body. Plus a
registry-ordering fact that was never written down — that `org-blocks-ref-diverge`
sat ahead of the editor rows and silently outranked them on every co-fired line.

## Remedy

PARTIAL — classification fixed, hybrid open.

- `org-blocks-ref-diverge` narrowed to `field deltas \(0\):`.
- Registered as `ref-diverge-content-text-drift` (`known-red`, UNOWNED),
  patterned on a content delta with both sides non-empty, and placed AFTER
  `editor-text-mirror` so the 44 co-fired lines now classify under that family
  where they belong.
- `editor-text-mirror`'s Evidence cell records the block-side co-fire and the
  measured split (44 of this shape + 8 of `ref-diverge-content-ref-empty` = 52
  lines it now receives).
- Pinned at x1 by the current-format fixture corpus
  (`fixture-logs-2026-09-19/`); inversion-verified.
- The pattern was TIGHTENED after adversarial verification. As first written it
  constrained only the ref side, so `content: sut="" ref="<text>"` — block LOSS,
  the SUT dropping content the reference still holds — classified here as a
  drift, contradicting this row's own "both sides non-empty" scope AND the
  sibling entry's claim that the direction stays novel. Both sides are now
  required non-empty, and the pattern is anchored on `field deltas \(1\):` plus
  the delta line.
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
- Escaped content is DELIBERATELY novel, for the reason given in
  [[2026-09-19-ref-diverge-content-ref-empty]]: `|` cannot appear in a Match
  pattern, so "escaped-or-plain" is inexpressible. Errs safe.

**RATIFICATION OWED** — triaged, not yet ratified by Martin.

Honest caveat on this row's weight: it has 45 lifetime observations as a
family, but only ONE that it claims once ordering is applied, and that one is a
hybrid. If the hybrid is not seen again, the right end state is probably to
delete this row and let the editor-mirror owner absorb the shape, rather than
keep a row alive for a single payload.
