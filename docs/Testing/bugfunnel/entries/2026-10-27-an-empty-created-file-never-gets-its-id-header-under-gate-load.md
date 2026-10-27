---
id: 2026-10-27-an-empty-created-file-never-gets-its-id-header-under-gate-load
date: 2026-10-27
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Under gate load, an empty org file created on disk is ingested as a page, but its `#+ID:`
  header write never lands, so the file stays 0 bytes and the page owns no file.
---

## Bug
`holon-integration-tests::two_instance_composed_pbt
joining_a_received_page_away_is_refused_and_leaves_it_placed` was red once in the extra
nextest leg of an integrate gate run (holon-space/holon-work#781). The oracle reported two
invariants at once:

- `inv-org-render-fixed-point`: disk `doc_0.org` is 0 bytes; the render from SQL is the
  43-byte `#+ID: <uuid>` header; the difference persisted for 5 s.
- `inv-every-page-has-its-own-file`: the page owns NO file.

Evidence: `docs/Testing/known-red-evidence/empty-created-file-header-never-written/gate-excerpt.log.zst`.
A 0-byte file next to a page that the store holds can lose data on a later write, so it is a
real defect, not only test noise.

## Root cause
UNMEASURED. The investigation stopped at the owed write-back:

- The test writes an empty `doc_0.org`
  (`crates/holon-integration-tests/src/pbt/transitions/create_document.rs`); the controller
  ingests it and mints the page.
- The file has no `#+ID:`, so `needs_id_writeback` forces a render
  (`crates/holon-filesystem/src/file_sync_controller.rs:7672`), and the render that differs from
  disk is put into `owed` for the write-back net (`file_sync_controller.rs:7855`).
- That write never landed within the 5 s budget. Candidate exits in `catch_up_write`
  (`crates/holon-filesystem/src/file_sync_controller/writeback.rs`): home not writable, the
  base-hash equality skip, the virtual-seed gate, quarantine, or a disk change. The share step
  of the test may also change the page's home before the write fires.

## Missing piece
The failing path runs in the test, but only heavy load exposes it: the test passed 25 of 26
gate extra runs, and three batches of 20 single-test runs on main were red 0, 3 and 0 times
(3 of 60); the red batch ran at load average about 64, each red with the same shape. No test forces the
timing window, so the red stays rare and looks like noise.

## Remedy
- Registered as known red `empty-created-file-header-never-written` in
  `docs/Testing/KeystoneKnownReds.md` (owner holon-space/holon-work#782). The fileless arm is
  matched by its own row `page-without-own-file`. The one-byte `org-render-echo-loop` shape and
  a double-homed page beside the empty file stay novel (`scripts/keystone-known-reds-fixture.sh`).
- Open: holon-space/holon-work#782 finds and fixes the dropped header write, with a
  deterministic red-first reproduction, and removes the row.
