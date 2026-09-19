---
id: 2026-09-19-ref-diverge-content-ref-empty
date: 2026-09-19
gap: ORACLE
secondary: ENVIRONMENT
status: PARTIAL
summary: >-
  The SUT holds block content the reference believes is empty, visible in
  `block_raw` and in SQL as well as in the org and matview projections, and the
  land gate absorbed every occurrence as `org-blocks-ref-diverge` pass-with-note.
---

## Bug

Found by the same registry audit as
[[2026-09-19-ref-diverge-parent-reparent]] — decoding what
`org-blocks-ref-diverge`'s overbroad Match pattern had been swallowing. **This
is the most reproducible and the most alarming of the three shapes it hid.**

15 panic lines across NINE distinct logs of the ten-log set named below.
3-LOG: 8 — seven in `triage/runs/A2-lib-and-composed.log` and one in
`land-w15b-nextest-1789664525.log`; the other seven of the fifteen live only
in logs outside that trio. Verbatim,
`land-w14-nextest-1789584904.log:12419`:

```
[inv-blocks-match-ref/org] fields diverge from reference
  inv-blocks-match-ref/org: 9 blocks, reference: 9 blocks
  only in inv-blocks-match-ref/org (0): []
  only in reference (0): []
  field deltas (1):
    block:vocab-declared: content: sut="plan" ref=""
```

Every one of the 15 co-fires on FIVE arms: `inv-blocks-match-ref/org`,
`inv-blocks-match-ref/matview`, `inv-blocks-match-ref/block_raw`,
`inv-block-content/block_raw` and `inv-block-content/sql`. The content
disagreement is therefore present in the store and in SQL, not only in a
projection — the widest layer set of the three shapes.

`block:vocab-declared: content: sut="plan" ref=""` recurs identically in six
separate nextest logs (`land-w14-nextest-1789584904`, `…-1789593946`,
`…-1789634032`, `…-1789644096`, `land-w15b-nextest-1789664525`,
`land-w15b-nextest-1789672138`). `triage/runs/A2-lib-and-composed.log` carries
seven more on a different block: `:2515` `block:gen-11: content: sut="uta"
ref=""` and `:2670` `block:gen-11: content: sut="a" ref=""`.

Direction is load-bearing. The mirror-image case (`sut="" ref="<text>"`, the
SUT LOSING content the reference holds) has not been observed and is
deliberately left novel. **This required a fix:** as first written, the
`ref-diverge-content-text-drift` pattern constrained only the ref side, so the
block-LOSS direction classified there as a "drift" and this sentence was false.
Both content patterns now require their stated side non-empty, and a synthetic
`sut="" ref="lost text"` line classifies NOVEL as claimed.

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

**The masking (this entry's gap).** Identical to
[[2026-09-19-ref-diverge-parent-reparent]]: the registry's
`org-blocks-ref-diverge` pattern anchored on the emitter headline
`fields diverge from reference`
(`crates/holon-pbt-core/src/block_compare.rs:159`) and so claimed every shape
the invariant can report, including content deltas that have nothing to do with
the set-membership excess the row documents.

**The underlying divergence: ROOT-CAUSED — a SUT-driver defect reachable only
from the PBT harness.** Neither of the two directions first guessed here was
right: it is not an oracle gap (the reference was correct) and not a store
defect (no unauthorised write ever happened — the authorised one never
happened at all).

The recurrence of one exact block and text (`block:vocab-declared` / `"plan"`)
is explained: it is not a seeded proptest path but a fixed gherkin fixture.
The failing case is `catalog_suite
logseq_parity_replay::logseq_parity_corpus_replay`, scenario `The document's
own #+TODO: vocabulary decides which keywords promote`
(`crates/holon-integration-tests/tests/fixtures/logseq-parity/tasks.feature:163`),
which is on the land gate's sanctioned known-red exclusion list and so was
never looked at.

The mechanism:

1. `NavigateFocus` runs `ReactiveEngine::spawn_caret_seat`
   (`crates/holon-frontend/src/reactive.rs:3184`), which resolves the
   document's first caret target — `block:vocab-declared` — arms
   `caret_seed = (vocab-declared, 0)` and focuses it **without opening an
   editor**.
2. `FocusEditableText` clicks that block. The headless driver placed a caret
   only when the click CHANGED the focused block, so the already-focused
   target never got one and the nav seed stayed armed.
3. The first backspace adopted the stale seed. At offset 0 backspace is the
   structural arm, so all four keystrokes dispatched `join_block`, found no
   merge target under a Page parent, and no-op'd. Content stayed `"plan"`.

Measured, not inferred — probe output at `lane-logs/repro3-probe.log:1026`:
`tracked=None armed_seed=Some(0)` with `current_text="plan"`, and
`[inv-sql-budget] DeleteBackward … writes=0/0` alongside four
`INSERT INTO operation` rows.

Production GPUI cannot reach this: `grab_focus_and_seed_caret`
(`frontends/gpui/src/views/editor_view.rs:1237`) applies the seed at the mount
navigation triggers and **consumes** it, and a GPUI mouse click places the
caret directly via `set_cursor_position` rather than through a seed. The two
shipped app drivers (`TuiUserDriver`, `GpuiUserDriver`) each implement their
own `click_entity` and never delegate to the headless one, so no app path
reaches it. The divergence is therefore a prod-vs-test parity defect in the
harness's SUT driver.

## Gap argument

Two escapes are recorded here, and the litmus questions separate them cleanly.

**Primary ORACLE — the masking.** The invariants did their job: three of them
fired, loudly and every run. What failed is the layer that reads their output.
The classifier claimed the panic for a row documenting a different shape, so a
firing invariant produced no actionable signal for nine logs. This is the
oracle layer failing to distinguish, not a generator or a wiring gap.

**Secondary ENVIRONMENT — the underlying divergence.** Not COVERAGE: the
interaction was generated, deterministically, every run. Not ORACLE: the
invariant fired the moment the state was reached. It is the ENVIRONMENT
litmus in its inverse form — the failing code path exists *only* in the test's
wiring, because the headless driver models navigation-then-click differently
from the way GPUI mounts an editor and retires its caret seed. The skill's
natural remedy for ENVIRONMENT is "make test and prod more similar", which is
exactly the fix: the editor-open path now places the caret and consumes the
seed, as the GPUI mount does.

Not FALSE-ALARM. The reference asserted something the product does promise —
backspace deletes a character — and the scenario's own `Then` steps encode it.
A real component misbehaved; it simply sits in harness-reachable code rather
than on an app path.

## Missing piece

The same one: a Match pattern that reads the diff body, which has printed
`field deltas (N):` since `render_block_diff` landed. Additionally the
2026-07-31 fixture corpus predates that renderer, so the pattern-drift guard
had no way to notice the overbreadth — its 123 archived panics contain no
`field deltas` text at all.

## Remedy

PARTIAL — classification fixed, driver defect fixed, known-red row still
standing pending a full-depth soak.

- `org-blocks-ref-diverge` narrowed to `field deltas \(0\):`.
- This shape registered as `ref-diverge-content-ref-empty` (`known-red`,
  UNOWNED), with a pattern anchored on the field name AND the direction
  (`ref=\\"\\"`), so the reverse direction stays novel.
- Placed after `editor-text-mirror`: 8 of the 15 also carry
  `inv-editor-text/mirror` and keep classifying there as the block-side view of
  that family. The remaining 7 are what this row claims.
- Pinned at x7 by the new current-format fixture corpus
  (`fixture-logs-2026-09-19/`); inversion-verified.
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
- Escaped content is DELIBERATELY novel: the value class is `[^\]`, so content
  holding a quote, backslash or newline matches neither content row. Expressing
  "escaped-or-plain" needs regex alternation and a `|` cannot appear in a Match
  pattern (it is the registry table's column separator). The failure direction
  is safe — an unclassified red is triaged, never silenced.

### The driver defect (lane `content-ref-empty-rca`)

Fixed in two places, both in `holon-frontend`:

- `HeadlessEditorMirror::seed_for_click` now calls `engine.consume_caret_seed`
  after placing the caret. Placing a caret retires the seed, exactly as GPUI's
  mount does.
- `ReactiveEngineDriver::seed_focused_editor` — the editor-open path — routes
  through `seed_for_click` instead of `reset_editor_from_authority`, so opening
  an editor places the caret rather than only re-seeding the buffer.

No hand-authored regression was appended: the lock is the existing corpus
scenario, which flips red → green. Red `lane-logs/repro1-baseline.log`
(`1 test run: 0 passed, 1 failed`), green `lane-logs/repro5-narrow-fix.log`
(`2 tests run: 2 passed`, run together with
`split_block_stale_display_regression` because a broader first attempt — which
dropped the already-focused guard in `click_entity` outright — reddened that
test with `reference model cursor_byte=0, SUT tracked caret=5`; the narrow fix
keeps both green).

GPUI seed ordering was checked, not assumed: the windowed overlay calls
`seed_focused_editor` on the shared engine after a real GPUI click, so the
consume could in principle retire a seed the GPUI mount had not yet applied.
It cannot. `grab_focus_and_seed_caret` runs synchronously inside the mount
(`editor_view.rs:756`) and on focus arrival (`:1116`), both before the click
returns, and a click places the caret directly rather than arming a seed. The
50 caret/seed/editor tests in `holon-gpui --features pbt` show 46 passed / 4
failed with the fix versus 45 passed / 5 failed on the base — the same 4
pre-existing failures both sides, no regression.

**RATIFICATION OWED** — triaged, not yet ratified by Martin.

Next steps:

1. Retire the `ref-diverge-content-ref-empty` known-red registry row and drop
   `logseq_parity_corpus_replay` from the land gate's sanctioned exclusion
   list, after one full-depth keystone sweep confirms the shape is gone.
2. `triage/runs/A2-lib-and-composed.log` carries seven more instances on
   `block:gen-11` (`sut="uta"`, `sut="a"`) from a different suite. Not
   investigated; they may or may not share this cause.
