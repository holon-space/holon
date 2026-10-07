---
id: 2026-10-08-degraded-boot-refuses-an-org-file-without-ids
date: 2026-10-08
gap: COVERAGE
status: FIXED
summary: >-
  When the recorded file state cannot be read at boot, an org file whose
  headlines carry no `:ID:` is refused whole, although no other file shares
  its name: its page is absent for the whole session.
---

## Bug
Found by the verifier osb-verify-4 (Defect 2) in a review of the
org-scan-boot lane (D108.a), change wxtknuwz. Introduced by that change's
load-failure guard.

The guard refused a file whose name chain names a page that holds blocks the
file does not declare. The org parser gives a headline without `:ID:` a fresh
random id, so such a file never declares the blocks its page already holds,
and the guard refused it although it is the page's only home. Probe:
`lane-logs/osb-verify-4/probe-6.log` (`Notes.org` with `* One` / `* Two`).

## Root cause
Undeclared blocks were taken as proof of a second home. Only a file with the
same name chain can be that home, and the guard did not ask for one.

## Missing piece
The guard's only test used a vault where a same-stem recipe was present
(COVERAGE). No test booted a degraded session with a lone org file whose ids
are not on disk.

## Remedy
The guard refuses only when a same-stem file is on disk
(`same_stem_files`, `crates/holon-filesystem/src/file_sync_controller.rs:2720`;
guard at `:5279`) AND the page holds undeclared blocks. The disclosure names
the same-stem file.

Gap-closing rung: `crates/holon-integration-tests/tests/cook_vault_ingest.rs`
`an_org_file_without_ids_keeps_its_page_when_the_recorded_file_state_is_unreadable`.
Red `lane-logs/r7/g12-red-1.log` (refused `["Notes.org"]`), green
`lane-logs/r7/g12-green-1.log`, teeth `lane-logs/r7/g12-teeth-1.log`.
