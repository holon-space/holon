---
id: 2026-09-17-opcap-panic-elides-error-chain
date: 2026-09-17
gap: FALSE-ALARM
secondary: PERCEPTION
status: FIXED
summary: >-
  The keystone's fail-loud op writer rendered an anyhow error with `{e}`, so the
  panic message carried only the outermost context; the classifier could not
  match the read-only-format refusal against either cooklang registry row and
  reported a correct, registered refusal as NOVEL, reding the land gate.
---

## Bug

The wave-14 land gate failed at its keystone step. `gate6.out`:

```
== [2/3] keystone full-depth sweep, classified ==
keystone-full: NOVEL reds, see …/land-w14-keystone-full-1789628084.class
  [novel] …log @ crates/holon-integration-tests/src/pbt/op_write_cap.rs:135:13:
      block/split_block operation failed: dispatch_intent_sync: block.split_block failed
  [novel] …log @ crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs:89:19:
      block/split_block operation failed: dispatch_intent_sync: block.split_block failed
successes: 1
NOVEL signatures (distinct, with occurrence counts):
    12 block/split_block operation failed: dispatch_intent_sync: block.split_block failed
[known-reds] FAIL: 12 novel panic(s), 0 known-red panic(s), 0 collateral (ignored).
```

Twelve panics, one signature, zero known-red matches — so the gate exits 1 and
the landing stops.

**The product is correct.** The panic is the read-only write-tier gate refusing a
`split_block` on a block homed in a `.cook` document: a read-only home is a write
boundary, and the refusal is the same behaviour described by
`2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded` and already
registered as the two `cooklang-read-only-*` rows in
`docs/Testing/KeystoneKnownReds.md`. Nothing here is a product defect; the red is
a reporting artifact.

The elision is visible in the message itself: it ends at
`dispatch_intent_sync: block.split_block failed` and never reaches
`cooklang is a read-only format`, which is what both registry rows anchor on.

## Root cause

Two halves, and the red needs both.

1. The panic at `crates/holon-integration-tests/src/pbt/op_write_cap.rs:135`
   renders the dispatch error with `{e}`. For `anyhow::Error`, `Display` prints
   the **outermost context only**; the alternative flag `{e:#}` joins the whole
   cause chain. The keystroke rungs in the same file (lines 289, 299, 329, 361,
   513) already use `{e:#}`, so the harness was inconsistent with itself: the
   same refusal was reported with its reason by one rung and without it by
   another.

2. `scripts/keystone-known-reds.sh` extracts the panic's message line and matches
   it with `grep -qE` against the registry's `Match pattern` column. Both
   cooklang rows anchor on text that lives *below* the outermost context —
   `block\.split_block failed: cooklang is a read-only format` and the bare
   `cooklang is a read-only format` — so an elided message matches neither and
   the signature is reported NOVEL.

Replaying the classifier's own match test over the two message shapes:

| message shape | registry rows matched |
|---|---|
| old, elided (`{e}`) | 0 |
| new, full chain (`{e:#}`) | 1 — `cooklang-read-only-write-refusal-any-op` |

## Missing piece

A perception gap: the harness held the reason and discarded it at the render
boundary. That is not a cosmetic logging detail, because signature matching *is*
the classifier's job — a panic site that elides its chain makes every registry
row anchored below the outermost context unreachable for that site, so a known
refusal and a genuinely novel failure shape become indistinguishable. The
misreport is also asymmetric in the dangerous direction: it inflates the novel
count (noisy, blocks a landing) rather than masking a regression, but the same
mechanism could hide one if a row's anchor happened to match the truncated
prefix.

## Remedy

FIXED. `{e}` → `{e:#}` at `crates/holon-integration-tests/src/pbt/op_write_cap.rs:135`,
plus the same elided render on the same dispatch path in
`crates/holon-integration-tests/src/pbt/sql_slice/components.rs:158`.

Verified end to end on both sides of the fix:

- before — `gate6.out`, 16-case sweep: `[known-reds] FAIL: 12 novel panic(s),
  0 known-red panic(s)`, signature `block/split_block operation failed:
  dispatch_intent_sync: block.split_block failed` (12x), exit 1.
- after — `just pbt general 16` (`lane-logs/pbt-general-16.log`), the same depth
  as the gate's sweep, which drew the refusal 30x: `[known-reds] PASS-WITH-NOTE:
  30 known-red panic(s), 0 novel, 0 collateral (ignored)`, exit 0, every panic
  classified as `cooklang-read-only-write-refusal-any-op`, e.g. `block/indent
  operation failed: dispatch_intent_sync: block.indent failed: Operation 'indent'
  on entity 'block' failed: cooklang is a read-only format: …keystone-recipe.cook
  is authoritative input and Holon ships no writer for it…`.

The two 1-draw runs (`keystone-smoke`, `just pbt general 4`) drew no refusal and
classified GREEN, which says nothing either way; the refusal needs a draw that
homes a block in the recipe document, so a proof of this fix must run at a depth
that reaches it.

The chain alone was enough to clear the false alarm: with it restored, the new
message shape matched `cooklang-read-only-write-refusal-any-op` (pattern
`cooklang is a read-only format`), a substring of the full message and the first
matching row. One anchor was then widened —
`cooklang-read-only-split-block-refusal`'s, to
`block\.split_block failed:.*cooklang is a read-only format` — to restore
split-block ownership, per the note below.

One attribution nuance, fixed with the same lane: the full chain interposes
`Operation 'split_block' on entity 'block' failed: ` between `block.split_block
failed` and `cooklang is a read-only format`, so the `split_block` row's original
anchor (which required those two segments contiguous) no longer matched and
`split_block` firings fell through to the `any-op` row — the sibling's notes say
it was registered precisely so `split_block` firings keep attributing to the
older row. Widening that one anchor restores the intent: `split_block` firings
land on `cooklang-read-only-split-block-refusal`, every other op name on the
`any-op` catch-all.
