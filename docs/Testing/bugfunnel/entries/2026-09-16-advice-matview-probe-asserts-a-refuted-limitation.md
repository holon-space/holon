---
id: 2026-09-16-advice-matview-probe-asserts-a-refuted-limitation
date: 2026-09-16
gap: FALSE-ALARM
secondary: null
status: OPEN
summary: >-
  `probe_ivm_shape_findings` asserts that IVM IGNORES the in-matview suppression
  anti-join; the assertion now fails, so the documented limitation no longer
  holds and the probe is red on main with no product defect behind it.
---

## Bug

`holon-advice::matview_build probe_ivm_shape_findings` fails at
`crates/holon-advice/tests/matview_build.rs:300`:

```
    assertion `left == right` failed: IVM ignores the in-matview suppression
    anti-join → suppression MUST be read-time
      left: 0
     right: 1
```

Found by the wave-14 land-gate triage
(`/tmp/holon-land-w14-1789580360/triage/TRIAGE.md` row 1), which classified it
PRE-EXISTING-ON-MAIN with no registry row anywhere in `docs/Testing`. It fails
at 0.03 s in both trees — A `A2-lib-and-composed.log`, B
`B2-advice-tui-handauthored.log` — and the B baseline is main `be08291ccf1b`, so
no lane caused it.

The test is a two-part probe of IVM's behaviour, and both parts are deliberate:

- FINDING 1 (GOOD): an overlap matview's self-join IS maintained correctly.
- FINDING 2 (BAD): the in-matview suppression `LEFT JOIN … WHERE s.lesson_id IS
  NULL` anti-join is IGNORED by IVM — the suppressed lesson stays present, and
  the comment records why: "This is why suppression is read-time."

FINDING 2 now fails, and the only thing its assertion checks is
`still_present.len() == 1` — that the suppressed lesson `lA` is STILL there. The
matview no longer returns it.

## Root cause

No product defect is demonstrated. The assertion encodes an observed engine
limitation as an expectation, so when the engine's behaviour changed the probe
went red while nothing about the product got worse — if anything the change is
an improvement, because the read-time suppression workaround the comment
justifies may no longer be necessary.

The probe is also not diagnostic, which is the part worth fixing: it cannot
distinguish "the anti-join is now honoured" from "the matview now returns
nothing at all". Both make `still_present.len()` zero. So the red does not say
which of the two happened, and no entry can claim the improvement either.

## Missing piece

Nothing in the product. What is missing is on the test side: an
engine-characterization probe pinned to a limitation is a snapshot, not a
property, and nothing re-validates it when the engine or the Turso fork moves.
It also asserts the ABSENCE of a row, which a broken matview satisfies for free
— so the probe passes and fails for the same observable.

`gap: FALSE-ALARM` — counted on its own line and excluded from the four-class
escape distribution, recorded rather than dropped, because the refutation is
itself the useful artifact: whoever next reads the "suppression is read-time"
decision needs to know its premise is no longer reproducible.

## Remedy

OPEN. Two outcomes, and the choice needs a measurement this lane did not make:

1. If the anti-join is genuinely honoured now, delete FINDING 2 and re-open the
   read-time-suppression decision as a possible simplification (a performance
   win, not a bug).
2. If the matview returns nothing, FINDING 2 is hiding a real regression and
   becomes an ORACLE-gap bug of its own.

Settling it needs a probe that asserts a POSITIVE count on a second anchor the
suppression does not touch, so an empty matview and a working anti-join can be
told apart. Until the probe is corrected or deleted, main carries this red.
