---
id: 2026-10-07-unsettled-claimant-retried-every-tick-undisclosed
date: 2026-10-07
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A file whose claimant cannot be stat'ed (for example an unreadable claimant
  directory) is left out of the store and re-attempted on every discovery tick
  forever, while the UI shows no condition: the file is simply absent.
---

## Bug
Found by the verifier osb-verify-3 (finding D6) in a review of the
org-scan-boot lane (D108.a), commit e16cec10. Inherited, not a regression:
before e16cec10 the same case returned `Ingested` and cleared a prior refusal.

When a file names a document (by `#+ID:` or by its name chain) and the stat
of the file that claims that document fails with anything except NotFound,
the ingest returns `IngestOutcome::UnsettledIdentity`
(`crates/holon-filesystem/src/file_sync_controller.rs:4999`, `:5248`).
`poll_new_files` records no quarantine for that outcome (`:8594`), so the file
is re-attempted on every tick. `disclose_unsettled_identity` (`:2565`) logged
ERROR once and DEBUG after that, and never raised a condition. The two
duplicate-id refusals and the shared-name-chain refusal do raise one.

## Root cause
`disclose_unsettled_identity` was written as a log-only disclosure. It did not
call `raise_ingest_refused_banner`, so a whole page stayed out of the store
and the only signal was in the log.

## Missing piece
- No keystone transition makes a claimant's stat fail, so the state cannot be
  generated (COVERAGE).
- No test asserted that an `UnsettledIdentity` file is disclosed. Only the
  embedded-id arm had a test, and it checked the outcome, not the disclosure
  (ORACLE).

## Remedy
`disclose_unsettled_identity` now raises the ingest-refused banner
(`file_sync_controller.rs:2589`). The existing `ingest_recovered` lifts it when
the file ingests or is deleted.

Gap-closing rung:
`crates/holon-orgmode/tests/sync_controller_mutation_pbt.rs`
`duplicate_doc_id_tests::a_name_chain_whose_claimant_cannot_be_stated_is_disclosed_until_it_settles`
(the claimant directory is set to mode 000; it also covers the name-chain arm,
which had no test). It asserts one banner that names both paths, and that the
banner lifts after the file ingests. Red `lane-logs/r6-d6-red-1.log`
("Raised: []"), green `lane-logs/r6-d6-green-1.log`, teeth
`lane-logs/r6-d6-teeth-1.log` and `lane-logs/r6-d6-teeth-2.log`.

The keystone still cannot generate a failing stat. That is left open.
