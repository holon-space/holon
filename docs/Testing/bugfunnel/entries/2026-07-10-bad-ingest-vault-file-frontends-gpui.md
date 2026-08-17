---
id: 2026-07-10-bad-ingest-vault-file-frontends-gpui
date: 2026-07-10
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  One bad-ingest vault file (Frontends/GPUI.org: `update_in_tree parent block
  not found` for a same-file `:ID:` parent defined EARLIER in the file) PANICS
  OrgMode startup on a tokio worker; app keeps running healthy-looking with
  file sync DEAD — subsequent vault edits silently ignored, no UI banner
source_line: 840
---

## Bug

One bad-ingest vault file (Frontends/GPUI.org: `update_in_tree parent block
not found` for a same-file `:ID:` parent defined EARLIER in the file) PANICS
OrgMode startup on a tokio worker; app keeps running healthy-looking with
file sync DEAD — subsequent vault edits silently ignored, no UI banner

## Missing piece

real-vault file shapes/scale absent from test env;
swallowed-background-panic invariant exists only in PBT harness, not prod
surface

## Remedy

FIXED (systemic): per-file scan failure no longer early-returns before
arming the watch loop (holon-orgmode di.rs) — other files ingest, live edits
land; wiring.rs panic → loud ERROR + `OrgIngestFailed` degraded banner.
Regression `boot_scan_bad_file_survives`. OPEN residue: exact inner
same-file ParentNotFound not reproduced headlessly (string comes from
test-only LoroBlockOrdering; prod path differs) — needs sanitized GPUI.org
fixture + two-boot persistent-DB harness
