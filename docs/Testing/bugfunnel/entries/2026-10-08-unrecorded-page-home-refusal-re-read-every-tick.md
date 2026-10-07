---
id: 2026-10-08-unrecorded-page-home-refusal-re-read-every-tick
date: 2026-10-08
gap: COVERAGE
status: FIXED
summary: >-
  When the recorded file state cannot be read at boot, an org file refused for
  an unrecorded page home is re-read, re-parsed and logged at ERROR on every
  discovery tick (2 s) for as long as no file records the page's home.
---

## Bug
Found by the verifier osb-verify-4 (Defect 1) in a review of the
org-scan-boot lane (D108.a), change wxtknuwz. Introduced by that change's
load-failure guard.

The guard refuses the file with `RefusedWhileClaimed(Document(doc))`.
`poll_new_files` quarantines it with that claim, and the next tick asks
`claimant_still_holds` → `live_claimant_of`, which looks only at `doc_home`.
In a degraded session `doc_home` has no entry for the page unless a file of
this session ingested it, so the claim "does not stand", the once-per-path
disclosure is re-armed and the file is re-ingested and refused again. Probe:
`lane-logs/osb-verify-4/probe-5.log` (5 ERROR lines, 2 s apart).

## Root cause
The refusal reused the claim kind of a duplicate `#+ID:`, whose re-check
asks for a recorded home. The unrecorded-home refusal has none by definition.

## Missing piece
No test kept a degraded session idle past several discovery ticks with no
file recording the page's home (COVERAGE). The existing test let the recipe
ingest later, which records the home and hides the defect.

## Remedy
New claim kind `ClaimedId::UnrecordedHome`. `claimant_still_holds` keeps it
standing while the page's home is still unrecorded and a same-stem file is
still on disk (`crates/holon-filesystem/src/file_sync_controller.rs:2434`);
once a file records the home, the usual `live_claimant_of` check applies.

Gap-closing rung: `crates/holon-integration-tests/tests/cook_vault_ingest.rs`
`an_org_file_refused_for_an_unrecorded_page_home_is_reported_once` (the
recipe beside the org file is broken, so no file records the home). Red
`lane-logs/r7/g12-red-1.log` (4 reports, expected 1), green
`lane-logs/r7/g12-green-1.log`, teeth `lane-logs/r7/g12-teeth-1.log`.
