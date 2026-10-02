---
id: 2026-10-02-update-log-zero-filled-tail-refuses-the-load
date: 2026-10-02
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A run of zero bytes at the end of the Loro update log (power cut after a size-extending append)
  failed the load as damage, so the app would not boot until the store was deleted.
---

## Bug

After a power cut following a size-extending append whose data never landed (filesystems without
ordered data, such as ext4 data=writeback or f2fs), the log ends in zeros. A 12-byte or longer
zero run read as a record header whose checksum fails (`crc32` of four zero bytes is not zero),
so replay refused the whole log. Found by the adversarial verifier (lane-logs/dvulv-verify.md
defect 1, lane-logs/dvulv-scratch.log), before landing.

## Root cause

`replay` treated only a short tail as a crash artefact; a zero-filled tail of header size or more
fell into the "damaged, not cut short" branch.

## Missing piece

The crash-shape tests covered a truncated tail only, not a size-extended one.

## Remedy

`replay` drops a run of zeros from a record start to EOF as a torn tail (disclosed); zeros followed
by a record still fail. Tests `a_zero_filled_tail_is_a_torn_tail_dropped_and_disclosed` and
`zeros_followed_by_a_record_fail_the_load_naming_the_rebuild`; red lane-logs/dvul8-red.log, teeth
lane-logs/dvul8-teeth.log. The same change makes a 0-byte or partial-magic log beside a snapshot an
empty log (verifier defect 3, hardening, no entry).
