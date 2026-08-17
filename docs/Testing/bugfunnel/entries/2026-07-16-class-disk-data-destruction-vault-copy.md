---
id: 2026-07-16-class-disk-data-destruction-vault-copy
date: 2026-07-16
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  P0-class ON-DISK DATA DESTRUCTION (vault copy): boot writeback rewrote
  Projects.org deleting the entire `* Holon` subtree — 6,245 of 6,976 lines —
  and rewrote Journals.org dropping its render/src source blocks + day
  headings that also exist as `Journals/<date>.org` files;
  `check_writeback_lossless` / quarantine NEVER fired (0 log mentions); only
  symptom = `writeback_sibling_grounding: name_chain failed loud` ERROR storm,
  after which the write proceeded anyway
source_line: 818
---

## Bug

P0-class ON-DISK DATA DESTRUCTION (vault copy): boot writeback rewrote
Projects.org deleting the entire `* Holon` subtree — 6,245 of 6,976 lines —
and rewrote Journals.org dropping its render/src source blocks + day
headings that also exist as `Journals/<date>.org` files;
`check_writeback_lossless` / quarantine NEVER fired (0 log mentions); only
symptom = `writeback_sibling_grounding: name_chain failed loud` ERROR storm,
after which the write proceeded anyway

## Missing piece

real-vault shape (same-named subdir page `Projects/Holon.org` vs `* Holon`
heading; day-files vs day-headings) not in any harness; the tripwire WAS on
this path but its threshold tolerated the drop

## Remedy

FIXED. Root cause: the 783-block `Projects/Holon.org` subdir page collides
(same name-chain) with the `* Holon` heading in `Projects.org`; boot ingest
re-homes the subtree into a PROHIBITED page-under-non-page topology. On
`on_block_changed`'s re-render the subtree is absent, and the tripwire's
`writeback_sibling_grounding` calls `name_chain` on the absent blocks →
fails loud (749-error storm, `sync_ports.rs:206`). The `Err` arm merely
`continue`d (ungrounded), and the ungrounded-drop count fell under the 25%
mass-truncation threshold → the truncated file was written anyway. The
fail-loud error did NOT abort the write. Fix (`file_sync_controller.rs`): a
name_chain grounding failure now marks the drop UNRESOLVABLE, which
HARD-VETOES + quarantines the write independent of the threshold (one guard
covers both `on_block_changed` and `re_render_all_tracked`). Pinned
RED-first (A/B verified) by
`writeback_drop_of_prohibited_subtree_hard_vetoes_prod_name_chain`
(name_chain_error_propagation.rs, REAL name_chain assertion) +
`name_chain_failed_ungrounded_drop_hard_vetoes`
(incremental_org_writeback_smoke.rs, threshold-boundary). Prod/test gap
closed: prohibited-topology write-back drop now in the harness. Evidence:
/tmp/dogfood-0716-logs/vault-writeback-damage.diff. **ROUND-2/3 FOLLOW-UP
2026-07-18 (release build, warm boot, real vault):** the hard-veto
quarantine works as designed and now fires at boot on FOUR distinct files,
all page-under-non-page topology collisions — Projects.org (doc db147710),
Projects/Holon.org (cb7d94d4), Projects/Holon/Testing.org (7919093d),
Projects/Holon/_archive.org (ad1b9891); 1754 `name_chain failed loud` errors
clustered in the first ~44s of boot, each file's ~750-block walk <100ms in
release. Data is PROTECTED (writes REFUSED, disk intact) but these four
files now get NO write-back every session — the underlying
prohibited-topology collision (the ROOT cause) remains unresolved for them,
so DB↔disk edits to those subtrees never persist. Not a perf driver
(boot-only, sub-100ms; the 5–10s nav latency is the separate 2026-07-18
recursive-matview row). Tracked here as the correctness residue of the
protective quarantine — un-quarantining requires fixing the same-name
page-vs-heading topology collision at ingest.
