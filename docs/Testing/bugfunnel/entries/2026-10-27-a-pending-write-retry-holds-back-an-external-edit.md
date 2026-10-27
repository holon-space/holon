---
id: 2026-10-27-a-pending-write-retry-holds-back-an-external-edit
date: 2026-10-27
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  An external edit to a file whose block write-back did not land waited out
  the write's retry backoff before Holon ingested it, so for up to 100 ms the
  store kept the old text ("Call mom") instead of the user's edit ("Call dad").
---

## Bug
The hand-authored case
`hm4-an-external-edit-after-a-write-that-did-not-land-reverts-the-stores-edit`
(`hm4e.org`) was red in about 1 of 5 isolated runs
(`HOLON_HAND_AUTHORED_CASE=<case> just hand-authored`) and green in the full
replay. Class: content race under load. The red run had no catch-up after the
`ExternalLineEdit`; the green runs retried within the settle and ingested.

## Root cause
One auto catch-up transition did both legs of the per-file write-back net:
the ingest of a file that moved and the write of a store that moved
(`crates/holon-filesystem/src/writeback_net.rs:154-168` on the base). The
block write-back that the armed disk failure refused left a retryable mark and
a `RetryPending` of 100 ms on that transition. The executor checks `RetryPending`
before it evaluates the guard or the input
(`crates/holon-net/src/exec.rs:2362`), so the new `OnDisk` that `ReadDisk` put
for the external edit did not enable the ingest until the backoff expired.
`on_file_changed` then returned `Ok(Ingested)` although no ingest ran
(`crates/holon-filesystem/src/file_sync_controller.rs:3482-3485`), so the
harness's `await_ingest` and its settle checked the store before the retry
tick. Not the crash-restore owed-write mechanism of
`store_base_for` (`crates/holon-filesystem/src/file_sync_controller.rs:8020`):
the base is correct, only the ingest is late.

## Missing piece
A deterministic ordering of "write fails, then the file changes" inside one
backoff window. The keystone reached it only when the replay ran faster than
100 ms of real time; under load the backoff expired first and the case went
green.

## Remedy
The net splits the catch-up into `CatchUpIngest` (the file left the base) and
`CatchUpWrite` (the store left the base over a file at its base), each with
its own retry, so a write's backoff never holds back an ingest (ADR 0032
§9.5: a new file content starts from zero tries). `net_with_backoff` lets a
test hold a retry pending for an hour, which makes the losing ordering
deterministic:
`an_external_edit_after_a_write_that_did_not_land_is_ingested_at_once` in
`crates/holon-orgmode/tests/writeback_compare_and_rename.rs`, red on the base
in every run. The net unit test
`a_pending_write_retry_does_not_hold_back_the_ingest_of_new_file_content`
pins the split.
