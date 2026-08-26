---
id: 2026-08-24-write-org-file-sql-reads-ceiling-flake
date: 2026-08-24
gap: ORACLE
secondary: null
status: UNCLASSIFIED
summary: >-
  `WriteOrgFile` breached its `inv-sql-budget` ceiling (31 dedup reads against
  ceiling 30) — 6 reads above the highest sample the budget's own calibration
  comment ever measured (25), so this is NOT a zero-slack/noise-at-the-edge
  case. A/B rerun (both arms green) clears the landing lane; whether the
  excess is a real redundant-read regression or an uncalibrated shape is
  undiagnosed.
---

## Bug

`inv-sql-budget` reported `WriteOrgFile` at 31 dedup SQL reads against an
expected ceiling of 30 — a one-read overshoot against the ceiling, but see
below — during a keystone run on 2026-08-24. This is a distinct transition
from the sibling entry `2026-08-24-typechars-sql-read-budget-exceeded` (which
covers `TypeChars`, found the same day by a different lane's observe-mode
draw); both are `inv-sql-budget` sightings from the same day on different
transitions with no shared root cause established. The raw log for this
`WriteOrgFile` case did not survive a session restart; only the
orchestrator's adjudication note (signature, method, verdict, date) remains.

The ceiling itself is real and arithmetically confirmed:
`crates/holon-integration-tests/src/pbt/transitions/write_org_file.rs:365`
expects `cdc_drain_floor(docs) + 22` with `tolerance: 5`, giving 30 at the
shape implied by the adjudication note (docs>1: 25 base + 5 tolerance).

## Root cause

Not diagnosed. Adjudicated DRAWN by A/B: rerun at the landing lane's tip and
at the base arm both came back green (no ceiling breach), so the overshoot
did not reproduce on either side of the change under test.

**Correction of the original filing.** The first draft argued the breach was
load noise because the ceiling "has essentially no slack." That is
contradicted by the budget's own calibration comment
(`write_org_file.rs:357-363`): "Dedup reads 9-25 over 9 samples (9 ×6, 16, 18,
25; all d=4)." Against a ceiling of 30, the worst sample the calibration ever
measured (25) has 5 reads of headroom, and the modal sample (9) has 21. A
31-read run is **6 reads above the highest sample ever measured**, not a
one-read wobble at a zero-slack edge — the "essentially no slack" premise was
wrong, and the load-noise conclusion built on it does not hold as stated.

That leaves the ORACLE primary classification without a confirmed reason: a
result 6 above the historical maximum is at least as consistent with a real
redundant-read excess on a shape the 9-sample calibration never covered
(all 9 samples were `d=4`; whether the failing run's document count matched
that is unknown) as with any kind of noise. Neither is confirmed without the
per-statement read breakdown.

**Second correction.** The original filing stated `KeystoneKnownReds.md` has
"two current `inv-sql-budget` rows" (`PinBlock` and `DeleteBackward`,
inherited verbatim from the `TypeChars` sibling entry, where it was true when
written). There are now three: `pinblock-unrendered-target` (:116),
`delete-backward-merge-budget` (:119, `fixed-pending-soak`), and
`deletebackward-sql-reads-budget` (:120, added 2026-08-25, `known-red`).
None of the three names `WriteOrgFile`, so the registry still carries no
matching row for this transition — that part of the original claim stands.

## Missing piece

No per-statement read breakdown (which queries ran, how many times each) was
captured for the failing run, and its document count is unknown — both are
needed to tell whether the extra reads are the SAME statement re-executed (a
real redundant-execution bug, in the `#15` roster's family) or a shape the
9-sample calibration simply never covered (the calibration is `d=4` only;
a higher document count is already priced into the ceiling via
`cdc_drain_floor(docs)`, but if that pricing itself is wrong at some doc
count, this is where it would show).

## Remedy

OPEN in the sense that the mechanism is undiagnosed and the ceiling's
correctness at this shape is unconfirmed. UNCLASSIFIED rather than NOTED: the
A/B clears the landing lane, but the refuted "no slack" rationale removes the
only reason this was filed as an unremarkable flake, and 6-over-the-historical-
maximum is a large enough excess to warrant deliberate follow-up, not a
shrug. Not added as a KnownReds row, for the same reason
`2026-08-24-typechars-sql-read-budget-exceeded` declined to: a single
ad-hoc breach with no per-statement breakdown is not yet a characterised
family, and registering one now would launder an untriaged signature into
"expected". If `inv-sql-budget` breaches on `WriteOrgFile` again, capture the
statement-level read breakdown and the document count before they rotate out.
