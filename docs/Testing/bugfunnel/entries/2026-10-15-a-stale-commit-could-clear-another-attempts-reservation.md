---
id: 2026-10-15-a-stale-commit-could-clear-another-attempts-reservation
date: 2026-10-15
gap: FALSE-ALARM
secondary: null
status: NOTED
summary: >-
  commit_consumption clears a reserved slot without checking which attempt
  reserved it; the real executor never reserves, so no attempt can clear
  another's reservation.
---

## Bug

Suspected by a code audit of the revision-2 executor work
(holon-space/holon-work#322, task E2), with no test behind it:
`Executor::commit_consumption` clears the slot whenever `Slot::reserved` is
set, and does not check which attempt set it. The suspicion was that a stale
commit could clear the reservation of a token another attempt had taken.

## Root cause

There is no product defect. `Slot::reserved` is written at exactly one place,
`Executor::start` (`crates/holon-net/src/exec.rs:2226`), and only on the
`Twin::HiddenReservation` branch — the deliberately broken executor variant
that exists so the property test of this rule shows red for its own reason.
The real executor TAKES the token out of the slot instead
(`exec.rs:2229`), so the place is empty and `decide` cannot find a token for a
second attempt of the same (place, subject). Two attempts therefore never hold
one slot, and `reserved` is always `false` outside the twin. The reference
model has no reservation at all (`crates/holon-net/tests/exec_support/model.rs`,
`Slot`), and the differential driver compares the full marking after every
operation, so a reservation that survived a commit would already diverge.

## Missing piece

Nothing. The field's doc comment said only "Only the hidden-reservation twin
sets it", which did not state the invariant that makes the owner check
unnecessary.

## Remedy

NOTED, task closed with the reason above. `Slot::reserved`'s doc comment now
states the invariant: no two attempts hold one slot, because a non-twin `start`
takes the token out.
