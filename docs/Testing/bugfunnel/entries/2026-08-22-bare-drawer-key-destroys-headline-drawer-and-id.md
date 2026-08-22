---
id: 2026-08-22-bare-drawer-key-destroys-headline-drawer-and-id
date: 2026-08-22
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A bare `:KEY:` line (no trailing space) in a HEADLINE property drawer destroys
  the whole drawer including `:ID:`, so the block silently gets a fresh UUID on
  ingest
---

## Bug

A headline property drawer containing a value-less `:KEY:` line loses the ENTIRE
drawer, not just that line — including the `:ID:` that carries the block's
identity. The block is then minted a fresh UUID at ingest, so every inbound link
to it dies and the next write-back rewrites the file's identity. Silent: no
error, no warning, a correct-looking page.

Found by the verifier (verify-2b1) while reviewing Increment 2b.1's
certification work — outside any automated test, hence this entry. Not caused by
that increment; it is pre-existing behaviour the certification probing walked
into.

Holon's own renderer cannot produce the trigger: it always emits `:KEY: ` with
the required trailing space (`crates/holon-org-format/src/models.rs:202`). The
trigger is reachable from any hand-authored or Emacs-written vault file, where a
value-less drawer key is ordinary org.

## Root cause

Independently reproduced (`lane-logs/F3-probe.log`), two fixtures differing by
one trailing space:

```
:PROPERTIES:      :ID: h-1  :A: x  :B: <space>   ->  id=block:h-1
                                                     A=Some(String("x"))
                                                     keys=[A, ID, _drawer_order, level, sequence]

:PROPERTIES:      :ID: h-1  :A: x  :B:           ->  id=block:c0d13863-50d6-…
                                                     A=None
                                                     keys=[ID, level, sequence]
```

The second case loses `:A:` as well as `:B:`, and the id is a freshly minted
UUID rather than the authored `h-1` — so the whole drawer was discarded, not
just the offending line.

The mechanism is upstream of Holon's own filtering: `extract_properties`
(`crates/holon-org-format/src/parser.rs:1124-1141`) filters nothing, so a pair
reaching it would survive. The drawer never reaches it — orgize's
`headline.properties()` does not yield the drawer at all once it contains a
value-less key. Same root as the `empty_string: dropped` clause certified in
2b.1, but strictly worse: an empty VALUE loses one key, an absent value loses
the drawer.

The file-level drawer has a separate, hand-rolled reader (`parse_drawer_line`,
`crates/holon-org-format/src/parser.rs:192-203`) which explicitly tolerates a
value-less key, so the two levels disagree. The verifier reports the file-level
path handles it correctly; my own probe did not discriminate that half (the
document id is path-derived, `file:x.org`, so it cannot show drawer loss), so
the file-level claim is the verifier's measurement, not mine.

## Missing piece

**No generator emits a value-less drawer line.** The keystone's `WriteOrgFile`
transition writes fixed fixtures, and the one drawer-key proptest in that file
(`crates/holon-integration-tests/src/pbt/transitions/write_org_file.rs:466-480`)
always formats `:{key}: {value}` — a space and a non-empty value, by
construction. No transition in the catalog can produce the trigger, so no case
ever reached this state.

The oracle side is NOT the gap: had a case reached it, the block's id would
diverge from the reference model's `blk-a`, which the existing identity and tree
invariants would flag. This is generation-only, hence COVERAGE with no secondary.

## Remedy

OPEN — reported, not fixed, per the lane's scope (Increment 2b.1 is a
certification increment; fixing the org parser is not in it).

Closing it means, in order:
1. widen the vault-file generator so a drawer line may carry an absent value
   (that is the COVERAGE fix, and it should go red for the right reason on the
   identity divergence before any parser change);
2. then decide the parser's contract — most likely reconcile the headline path
   with the file-level path, which already tolerates a value-less key
   (`parser.rs:192-203`), rather than teaching a second tolerance.

Worth noting for whoever takes it: the same fixture class is what 2b.2 needs to
add the file-level drawer as a certification carrier, so the two pieces of work
share their fixtures.
