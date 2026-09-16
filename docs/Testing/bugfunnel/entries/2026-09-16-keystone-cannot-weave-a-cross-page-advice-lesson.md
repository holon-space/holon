---
id: 2026-09-16-keystone-cannot-weave-a-cross-page-advice-lesson
date: 2026-09-16
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Over the live MCP surface, `describe_ui` never shows the top advice lesson
  woven under its anchor; the assertion that catches it lives only in a side
  binary, and the composed keystone exercises no cross-page advice weave at all.
---

## Bug

`holon-integration-tests::advice_live_mcp_gate advice_live_mcp_gate` fails at
`crates/holon-integration-tests/tests/advice_live_mcp_gate.rs:325`:

```
    describe_ui: top lesson block:lesson-b must be woven under the anchor
    (it lives on a separate page, so a rendered row with this id is the weave).
    rendered ids=[…] ui={…}
```

Found by the wave-14 land-gate triage
(`/tmp/holon-land-w14-1789580360/triage/TRIAGE.md` row 23), classified
PRE-EXISTING-ON-MAIN. It fails in both trees — A 4.8 s, B 5.8 s — with the
identical signature (`A1-int-nonwindowed.log`,
`B1-int-tests.log:524`), and B is main `be08291ccf1b`.

The assertion is the whole point of the gate: `lesson-b` lives on a DIFFERENT
page from `anchor-a`, so a rendered row carrying that id can only be there
because the weave put it there. The ids and the whole `describe_ui` JSON are
dumped in the failure message, so the absence is evidenced rather than inferred.

## Root cause

UNATTRIBUTED. The capture fixes the observable and the surface (the embedded
MCP server's `describe_ui`, driven through `McpUserDriver`), not the mechanism.
Nothing in the log says whether the weave did not run, ran and produced a
different top lesson, or ran and was not rendered in the main panel.

Two things narrow it, and neither is a finding:

- A sibling assertion in the same family is already registered —
  `2026-08-26-advice-catalog-tests-nondeterministic` covers
  `catalog_suite`'s `advice_step4_red` failing on "woven advice row
  `block:lesson-b` is not in the reference expectation". But that entry's defect
  is nondeterminism under the one-process `cargo test` harness, in a DIFFERENT
  binary. This one fails identically in A and B, so the two are not the same
  observation and are filed separately; whether they share a root cause is
  unestablished and is a question for whoever owns the weave.
- The scores differ in shape: the 08-26 dump scored `lesson-c` 2 and `lesson-d`
  1 with `lesson-b` absent, which is a ranking outcome rather than an empty
  weave.

## Missing piece

The composed keystone does not exercise this at all. `lesson-b` / `lesson_b`
appears nowhere under `crates/holon-integration-tests/src/pbt/`, and the
transitions directory has no advice or lesson transition, so there is no
generated case in which a lesson living on another page must surface under its
anchor. The only thing asserting it is a single-purpose side binary, which is
why a red on main could sit unowned: no composed invariant expresses "a lesson
from another page is woven under its anchor, and the weave is visible in
`describe_ui`".

The ORACLE secondary is the same absence seen from the other side — even if the
keystone drew the weave, no invariant in
`crates/holon-integration-tests/src/pbt/composed/invariants/` would judge the
cross-page case.

## Remedy

OPEN, and the first step is a decision, not a fix: determine whether the product
stopped weaving or the gate's expectation has drifted, because the two remedies
diverge from there. A `git log`/`jj log` bisect of `lesson-b`'s last known-green
run is the cheapest discriminator, and the 08-26 entry's lane notes are the
place to start.

The gap-closing work is separate and worth doing either way: a composed
transition that seeds a lesson on a separate page and an invariant that judges
the weave would put this under the keystone instead of under one binary.
