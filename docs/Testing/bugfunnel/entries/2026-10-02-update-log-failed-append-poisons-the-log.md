---
id: 2026-10-02-update-log-failed-append-poisons-the-log
date: 2026-10-02
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A part-way failed append (ENOSPC/EIO) to the Loro update log left a stump that the next save
  appended behind, so the load refused and both acknowledged saves were lost.
---

## Bug

`update_log::append` wrote a record with `write_all`; an error part-way left the partial bytes in
the file, and the next successful save appended behind them. The stump then sat mid-file, the load
failed with "the record header ... fails its checksum ... delete the Loro store", and the
acknowledged save after the stump was unreachable. Found by the adversarial verifier of the update
log landing (lane-logs/dvulv-verify.md defect 2, scratch log lane-logs/dvulv-scratch2.log), before
landing.

## Root cause

`append` opened the file in append mode, never compared the file length with the length the store
tracks in `Persisted::log_bytes`, and never truncated after an error.

## Missing piece

No test injected a failing write into the append path; the crash-recovery tests only shaped the
file after the fact.

## Remedy

`append` takes the length of the last acknowledged record, cuts a failed append back to it
(`sync_data`) before returning the error, and cuts any stump beyond it before appending; the store
discloses a cut stump as `LoroUpdateLogTailDropped`. Tests
`a_failed_append_cuts_its_partial_record_off_and_later_saves_survive` and
`a_stump_behind_the_acknowledged_log_is_cut_off_before_the_next_append` in
crates/holon-loro/tests/update_log_recovery.rs; red lane-logs/dvul8-red.log, teeth
lane-logs/dvul8-teeth.log.
