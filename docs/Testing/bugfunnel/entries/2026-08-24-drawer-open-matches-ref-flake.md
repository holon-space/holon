---
id: 2026-08-24-drawer-open-matches-ref-flake
date: 2026-08-24
gap: ENVIRONMENT
secondary: ORACLE
status: UNCLASSIFIED
summary: >-
  `inv-drawer-open-matches-ref` intermittently mismatched in a keystone draw.
  The invariant's origin entry (2026-08-16) proved the snapshot layer green
  and carries an explicit ENVIRONMENT residual, but that residual is about
  unverified PIXEL rendering, not the snapshot-layer comparison this
  invariant itself performs — so it does not by itself explain this
  sighting. A/B rerun (both arms green) clears the landing lane; mechanism
  undiagnosed.
---

## Bug

The `inv-drawer-open-matches-ref` invariant reported a mismatch between the
rendered drawer-open state and the reference model's expected drawer-open
state during a keystone run on 2026-08-24. No detail of the mismatched case
(which block, which drawer, the diff) survived a session restart — only the
orchestrator's adjudication note (signature name, method, verdict, date) is
available for this entry.

**Correction of the original filing:** this is not a new family.
`docs/Testing/bugfunnel/entries/2026-08-16-web-builder-ignores-open-closed-state.md`
is where `inv-drawer-open-matches-ref` was introduced
(`crates/holon-integration-tests/src/pbt/invariants/bodies/drawer_open_matches_ref.rs`),
proven red-for-the-right-reason then green (`5/5`, `3/3` engagement), and it
carries an explicit disclosed residual: "the ENVIRONMENT half is NOT closed:
the keystone asserts the SNAPSHOT carries the right open state, which is the
layer the bug lived at, but no arm renders a real browser, so the PIXEL claim
… remains unverified by any gate."

That residual is specifically about the unrendered-pixel half, not about the
snapshot-layer comparison the invariant itself performs — the entry states
the snapshot layer was proven green. This 2026-08-24 sighting is a mismatch
in that same snapshot-layer comparison, which the origin entry treats as
closed. The residual therefore does not directly explain this occurrence; it
establishes that this invariant's family has known open ground nearby
(pixel rendering), not that the snapshot check itself is known-flaky.

## Root cause

Not diagnosed. Adjudicated DRAWN by A/B: rerun at the landing lane's tip and
at the base arm both came back green — the mismatch did not reproduce on
either side of the change under test.

Two hypotheses, neither confirmed without the original case:

1. **ENVIRONMENT — settle-timing race.** The reference model and the rendered
   snapshot state are read at slightly different points relative to an async
   settle, so a transient window exists where they legitimately disagree
   before converging.
2. **ORACLE — invariant timing sensitivity.** The invariant samples state
   before a pending projection update lands, independent of any test/prod
   wiring divergence — a property of when the check runs relative to the
   mutation, not of what code path exists only in test.

## Missing piece

The specific case/seed and the actual before/after drawer-open values are
gone, so there is no way to distinguish the two hypotheses above, and no way
to tell whether this sighting relates to the origin entry's open pixel
residual at all. Making this attributable would need the invariant's failure
to log both the rendered and reference drawer-open values plus a
timestamp/tick relative to the driving action, captured before log rotation.

## Remedy

OPEN in the sense that the mechanism is undiagnosed. UNCLASSIFIED rather than
NOTED: the A/B clears the landing lane, but this invariant's family already
carries one disclosed open residual, and asserting a confident gap here
without knowing whether this sighting is a new snapshot-layer issue or an
artifact of that residual would overstate what is known. If
`inv-drawer-open-matches-ref` mismatches again, capture the case's log and the
invariant's before/after values before they rotate out, and attach them to
this entry.
