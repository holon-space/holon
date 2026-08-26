---
id: 2026-08-26-advice-catalog-tests-nondeterministic
date: 2026-08-26
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Two `catalog_suite` advice tests fail nondeterministically under the
  single-process `cargo test` harness — same code, different runs disagree on
  which one reds — so a full-suite result cannot attribute a red to a change.
---

## Bug

Found by agent exploration during the `fix-oracle-gaps` lane's landing review
(2026-08-26), not by any test. Two tests in `catalog_suite` red
nondeterministically when the suite is run through `cargo test` (all tests as
threads in ONE process):

- `advice_dismiss_prod_session_wiring::prod_session_dispatches_dismiss_advice`
  fails `dismiss_advice must append exactly one (anchor, lesson) row to
  advice_suppressed (left 0, right 1)`.
- `advice_step4_red::advice_step6_synthesis_weave_and_dismiss_green` fails
  `[inv-advice-rows-woven] woven advice row 'block:lesson-b' is not in the
  reference expectation (scored candidates: [("block:lesson-c", 2),
  ("block:lesson-d", 1)])`.

The escape is not either test's assertion — both assertions are correct. The
escape is that a full-suite red **cannot be attributed to a change**, which is
what a landing gate exists to do. This already produced one false attribution:
a verifier concluded the lane had caused the `advice_dismiss` red.

## Root cause

Measured on identical lane content, `cargo test -p holon-integration-tests
--features pbt --test catalog_suite`:

| runs | advice_dismiss | advice_step4_red |
|---|---|---|
| 6 (this session, incl. one under 20-way CPU load) | ok 6/6 | FAILED 6/6 |
| 2 (verifier, same lane content) | FAILED 2/2 | ok 2/2 |

Same code, opposite outcomes — so the outcome is decided by run-to-run
nondeterminism, not by content. `advice_dismiss` additionally passed 20/20 when
run alone (`--exact`), so the nondeterminism needs the concurrent suite.

Load is NOT the variable: the deliberately loaded run (20 spinners on 16 cores,
124s vs the usual 79s) reproduced the same outcome as the unloaded runs.

Two mechanisms are implicated and neither is yet proven:

1. `pick_two_blocks`
   (`crates/holon-integration-tests/tests/catalog_suite/advice_dismiss_prod_session_wiring.rs:38`)
   picks `ids[0]`/`ids[1]` out of `BlockSnapshot::iter_blocks`, which is
   `self.by_id.values()` over a **`HashMap`**
   (`crates/holon-core/src/storage/block_query.rs:112,156`). The pair is
   therefore an arbitrary draw that varies per process.
2. The write is `INSERT OR IGNORE` into `advice_suppressed`
   (`crates/holon/src/core/sql_operation_provider.rs:4052`). Its comment
   justifies `OR IGNORE` as PK-collision idempotence, but the table also carries
   `FOREIGN KEY (anchor_id) REFERENCES block_raw(id)`
   (`crates/holon-turso/sql/schema/advice_suppressed.sql`), and `OR IGNORE`
   swallows an FK violation just as silently. An anchor drawn from the
   projection snapshot but not (yet) present in `block_raw` would produce
   exactly the observed symptom: dispatch returns `Ok`, zero rows land.

Note the test's own doc comment is wrong on this point — it says
"`advice_suppressed.lesson_id` has an immediate FK into it". The FK is on
`anchor_id`; `lesson_id` is deliberately unconstrained (the schema comment
explains why).

Not reproduced on demand, so mechanism 2 remains a hypothesis. Under `cargo
nextest` (process per test) `advice_dismiss` passed in every observed run.

## Missing piece

No determinism at the seam where a test picks its subject: `pick_two_blocks`
draws from an unordered `HashMap` with no ordering and no proof that the drawn
anchor satisfies the FK the write depends on. Secondarily an oracle gap — the
production write can fail its FK and still report `Ok`, so nothing in the
system distinguishes "dismissed" from "silently dropped". A `dismiss_advice`
that inserts nothing is indistinguishable from one that worked, which is the
"silently degrades to look fine" case the repo's error philosophy forbids.

## Remedy

OPEN. Not fixed in the `fix-oracle-gaps` lane: the flake could not be
reproduced there (6/6 and 20/20 green), and fixing an unreproduced failure
would be guessing. Proposed, in order:

1. Make the subject selection deterministic and FK-valid: sort the candidate
   ids and pick the anchor from `block_raw` itself rather than from the
   projection snapshot.
2. Decide whether `INSERT OR IGNORE` may swallow an FK violation on this path.
   If not, split the concerns — keep PK idempotence, surface the FK failure —
   which is a production behaviour change and needs the `holon-feature`
   red-first treatment plus a ruling, since the schema comment records a
   deliberate data-loss tradeoff in the neighbouring `lesson_id` case.
3. Until then, treat a full-suite `catalog_suite` red on either advice test as
   unattributable: re-run, or attribute only via `cargo nextest`
   (process-per-test), which did not exhibit it.
