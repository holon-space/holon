---
id: 2026-09-16-keystone-create-under-a-read-only-focus-root-panics
date: 2026-09-16
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  The keystone's create-under-focus SUT panics when the focus root sits in the
  read-only recipe document, because the write-tier gate's refusal is a hard
  harness failure instead of a shaped expectation — the product is correct.
---

## Bug

`general_e2e_composed_pbt` (headless keystone) panicked at
`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:4213`:

```
    [SutBlockCreate::apply_create_under_focus] create_block_with_id(block:gen-8)
    under block:5f54ed1a-400a-8b5e-1e0e-9b62ad8370cd failed:
    dispatch_intent_sync: block.create failed: Operation 'create' on entity 'block'
    failed: cooklang is a read-only format: <tmp>/keystone-recipe.cook is
    authoritative input and Holon ships no writer for it, so this edit would live
    only in the store. Edit the file on disk to change it.
```

Capture: `.claude/worktrees/d128-fixture-peer/lane-logs/v128-smoke-true-1789585613/t9.log`,
a 10-run sample (`v128-smoke-sample-1789585495/s1..s8.log` plus
`v128-smoke-true-1789585613/t1..t10.log`). The `apply_create_under_focus … failed`
line occurs 16x in `t9.log` and nowhere in the other 17 runs of the sample, all
of them against the same focus root `block:5f54ed1a-…370cd`.

A second symptom of the same refusal, in the same run: the creation-affordance
bail at `crates/holon-frontend/src/user_driver.rs:724` —

```
    [SutBlockCreate::apply_create_under_focus] creation-affordance birth of
    block:<uuid> under block:5f54ed1a-…370cd did not land within 3s — the
    block.create dispatched by the focus edge never produced a row
```

— 4 lines in `t9.log` (lines 369, 398, 416), the fourth being the run's own
`Test failed:` restatement of the third. Three distinct births, one per attempt
to seat a caret on a creation affordance under the same read-only focus root.

**The product is correct in both symptoms.** The refusal is the write-tier gate
built for `2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded`
working as designed: a store-origin create whose owning document is homed in a
`WriteTier::ReadOnly` file must be refused. Nothing here is a product defect.

## Root cause

The keystone seats the focus in the read-only recipe document — `NavigateFocus`
can target any page, and the recipe page is in the page list — and then draws a
create under that focus. Two harness sites treat "the create lands" as
unconditional:

- `SutBlockCreate::apply_create_under_focus` resolves the created row with
  `.unwrap_or_else(|e| panic!(...))` (components.rs:4213, the `Some(id)` arm;
  :4223 the slot arm), so a dispatcher refusal is a hard panic.
- the creation-affordance driver waits 3 s for the newborn's row and bails when
  it never appears (user_driver.rs:724).

Neither is wrong about what it waits for. What is missing is a shape for the
refusal: the harness has no branch in which a create aimed at a read-only-homed
parent is expected to be refused, disclosed, and judged.

The sibling transitions already do this correctly and are the precedent to
copy: `attempt_read_only_edit.rs` and `attempt_ingest_compound_on_read_only.rs`
both aim a write at a read-only home and deliberately discard the outcome —
"Acceptance is the expected outcome and is judged by the invariant; a refusal
must reach it, not die in an assert here" — leaving `inv-read-only-home-refuses-writes`
to judge. `ApplyCreateUnderFocus` is the one write-shaped transition that was
never given that treatment.

## Missing piece

A refusal-shaped expectation. The interaction is generatable and IS generated —
the alphabet reaches it without help — so this is not a gap in what the keystone
can draw. What is absent is any invariant that could have flagged the refusal:
no invariant expresses "a create under a read-only-homed focus root either lands
or is refused with a disclosure", so the only possible outcome is the panic.

The COVERAGE secondary: `apply_create_under_focus` has no precondition on the
writability of its resolved parent, so the generator draws a create whose parent
cannot accept it. Either remedy closes the escape — a precondition excludes the
draw, or the transition admits the refusal — and the second is the one that
keeps the refusal under test.

A note on the classification, for whoever owns the convention: no product defect
is behind this red, which is the literal shape of `FALSE-ALARM` ("an oracle
stronger than the property under test"). It is filed as ORACLE/COVERAGE because
what is missing is a harness expectation, and that is a test-quality investment
the distribution should see. Reclassify to FALSE-ALARM if the team's convention
is that a harness-only gap is not an escape.

## Remedy

OPEN. The change is in the harness, not the product: give the refusal a shape.
Concretely, `apply_create_under_focus` should route through the same
attempt-and-judge shape as its siblings — dispatch, record the outcome, and let
an invariant judge whether a read-only-homed parent produced a disclosure
(`inv-conditions-match-ref` already judges the disclosure side for the other two
transitions) — and the creation-affordance driver should stop treating a 3 s
miss as fatal when the parent is known read-only-homed.

Not fixed in this lane: this is a docs/triage lane and touches no code.
