---
id: 2026-08-24-typechars-sql-read-budget-exceeded
date: 2026-08-24
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  TypeChars exceeds its dedup SQL-read budget at three measured shapes
  (16/17/18 against expected 7/9/11), with up to 31 redundant re-executions in
  one transition — an inv-sql-budget signature the KnownReds registry does not
  carry, so every sighting reads as novel.
---

## Bug

`inv-sql-budget` fails on the `TypeChars` transition in a wide composed
keystone draw. Three distinct shapes, all dedup reads against the transition's
expectation plus tolerance 5:

| dedup reads | raw reads | redundant re-executions | expected | ceiling |
|---|---|---|---|---|
| 18 | 49 | 31 | 11 | 16 |
| 17 | 35 | 18 | 9  | 14 |
| 16 | 21 | 5  | 7  | 12 |

Found while driving a wide draw for an unrelated reason (exercising the
observe-mode logging of `inv-no-declared-column-absent`), not by a gate:
`just pbt general 12`, 11 violations, all `TypeChars`, log
`.lane-logs/g2-observe-proof.log`.

NOT attributed to the change that was in the tree when it was observed. That
change (the fail-loud CDC parse path plus the declared-column oracle) issues no
SQL, which the verifier confirmed independently. Prior sightings exist: the
orchestrator's daytime flake census recorded `inv-sql-budget` `TypeChars` on
main-based trees before any of this night's landings.

## Root cause

Not diagnosed. The shape — dedup reads roughly 1.6× the expectation while RAW
reads run 2–3× the dedup count — matches the `#15` redundant-read family
(identical SQL re-executed within one transition), here in a transition the
roster has not yet characterised. The 31-redundant case is the sharpest: a
single keystroke transition re-issuing the same reads 31 times.

Two things make it worth its own entry rather than a footnote:

1. **The registry does not carry this signature.** `docs/Testing/KeystoneKnownReds.md`
   has exactly two `inv-sql-budget` rows — `pinblock-unrendered-target`
   (`PinBlock.sql_reads: … exceeds expected 17`) and
   `delete-backward-merge-budget` (`DeleteBackward.sql_reads: … exceeds
   expected 5`). Neither matches `TypeChars`. Under the registry rule a
   non-matching signature is NOVEL, so every future sighting costs another lane
   the same triage this one did.
2. **Prose mentions are not registry rows.** The `syn-real-mint` row's evidence
   paragraph mentions "two `inv-sql-budget`" among five smoke signatures. That
   is a measurement narrative, not a matchable row, and reading it as
   pre-existence cover is exactly the misattribution this entry exists to stop —
   it is the mistake this lane made and the verifier caught.

## Missing piece

An `inv-sql-budget` `TypeChars` row in the KnownReds registry, or the
expectation recalibrated against what typing actually costs. Neither exists, so
the family is invisible to the classifier while being live enough to fail a
12-case draw.

Whether the budget or the code is wrong is open. The `delete-backward-merge-budget`
precedent is a caution in one direction — there the NUMBER was the defect, an
oracle constant never calibrated against the path it was charging — and the raw
re-execution counts here are a caution in the other.

## Remedy

OPEN. Not fixed here: this lane's change is unrelated, the transition budget is
a measurement campaign rather than an edit, and a fix wants its own red-first
change.

Deliberately NOT recorded as a KnownReds row either. A registry row asserts a
characterised family with an attribution, and this has neither; adding one from
a single ad-hoc draw would launder an untriaged signature into "expected". The
entry is the honest artefact until someone measures it.

The A/B that would settle pre-existence beyond the census — the same draw at a
base rev — was not run: it needs a working-copy revert the observing lane held
no permission for.
