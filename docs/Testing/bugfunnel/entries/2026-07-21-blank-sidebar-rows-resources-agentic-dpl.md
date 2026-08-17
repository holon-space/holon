---
id: 2026-07-21-blank-sidebar-rows-resources-agentic-dpl
date: 2026-07-21
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Blank sidebar rows (Resources, Agentic DPL) persist after the ingest-heal
  fix — the heal runs only on the file-watcher reingest path
  (`process_external_change`), not on boot or unchanged files, so pre-existing
  empty-content doc-roots that went through the convert/delete title-less
  chain stay title-less on a normal boot (live: `Resources.org` =
  `block:a9163ed8`, `Projects/DBG/Agentic DPL.org` = `block:9464fbf0`, both
  content="", `Page`-tagged). Byte-identical `#+ID:`-only sibling files title
  fine — the discriminator is store state, exactly as the heal commit says,
  and existing store state is never re-healed on a normal boot.
source_line: 1081
---

## Bug

Blank sidebar rows (Resources, Agentic DPL) persist after the ingest-heal
fix — the heal runs only on the file-watcher reingest path
(`process_external_change`), not on boot or unchanged files, so pre-existing
empty-content doc-roots that went through the convert/delete title-less
chain stay title-less on a normal boot (live: `Resources.org` =
`block:a9163ed8`, `Projects/DBG/Agentic DPL.org` = `block:9464fbf0`, both
content="", `Page`-tagged). Byte-identical `#+ID:`-only sibling files title
fine — the discriminator is store state, exactly as the heal commit says,
and existing store state is never re-healed on a normal boot.

## Missing piece

no boot-time heal sweep (or heal on the initial boot-ingest path, not only
`process_external_change`); keystone has no boot-ingest-vs-filewatch path
split nor a reboot-against-pre-existing-empty-doc-root rung

## Remedy

FIXED 2026-07-22 (fix-title-heal-ingest, PR #78) — as a STORE-HEALTH PASS,
not a fast-path predicate (design ruling: encoding one degradation in the
skip predicate would leak the next degradation class through the same skip —
"two code paths where we should have one"). Diagnosis: boot and file-watch
both call `ingest_file`; on a byte-unchanged file the cold-boot fast-path
(`last_projection_hash` hash-match + `content_present_in_all_stores`) skips
ingest before the `#+ID` arm, so a degraded empty-content `Page` was
stranded every boot. Fix shape: (1) the fast-path certifies ONLY
byte-identity + store presence (reverted — no health reasoning). (2) The
heal is extracted to ONE
`FileSyncController::heal_title_less_doc_root(path)` implementation and
removed from `ingest_file`'s conditional. (3) An UNCONDITIONAL boot
store-health sweep `heal_title_less_doc_roots()` runs after the scan (in
`run_file_sync_controller`, alongside `materialize_missing_page_files`),
iterating the vault's org files and invoking the single heal per file —
robust to ANY store-degradation that empties a doc-root, not just this
class. (4) `on_file_changed` invokes the SAME heal for runtime (post-boot)
file-watch reingests (gated `!in_initial_scan` so the sweep owns boot). (5)
Residual empty-stem case (a file with no derivable title) WARNs by doc id
instead of re-leaving empty content silently; the disclosed render-side
`(untitled)` placeholder (`render_eval` `empty:` named-arg, PR #59) covers
that row visually. Red-first boot proof:
`idonly_title_heal.rs::idonly_root_page_heals_on_boot_store_health_sweep`
drives the sweep directly against a degraded empty-content `Page` (GREEN =
title re-derived; RED without the sweep = `recorded updates = []`). Two
file-watch heal tests stay green through `on_file_changed`. Real-vault copy
(`holon-pkm-copy`): `Resources.org`/`People/…`/`Projects/DBG/Agentic
DPL.org` are `#+ID:`-only on disk exactly as reproduced; the booted
`holon-run` DB shows those pages already titled, so the degraded
empty-content state exists only in the live vault — the unit test carries
that repro.
