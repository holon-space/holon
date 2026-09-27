---
id: 2026-09-27-markdown-declared-id-bypasses-the-id-rule
date: 2026-09-27
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A LogSeq `id::` (page or block) and an Obsidian `^anchor` were taken with
  `EntityUri::block(<text>)` unchecked: `block:abc` hit the double-scheme
  debug assert, `a b` and `café` panicked in URI parsing, and `a#b` became an
  id that no org id line keeps.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 5
(`lane-logs/groupA-r5-verify.md`, seam census). The round-5 report disclosed
the LogSeq page id; the block id and the Obsidian anchor had the same shape.

## Root cause
`crates/holon-markdown/src/logseq.rs` and `obsidian.rs` built block URIs from
file text directly, without the `DrawerId` rule the org carriers use.

## Missing piece
The Markdown tests used fixture UUIDs and ASCII anchors only.

## Remedy
`declared_block_id` (`crates/holon-markdown/src/build.rs`) parses a declared
id through `DrawerId` and refuses the file by name. A declared block can be
moved into an org file, so the same rule applies. Tests:
`crates/holon-markdown/tests/explicit_id_contract.rs`, and through the
controller `a_logseq_page_whose_id_names_a_scheme_is_refused_by_name`
(`crates/holon-orgmode/tests/page_id_with_a_scheme_is_refused.rs`: the file is
disclosed through `ingest_refused`, i.e. VaultIngestFailed); red
`lane-logs/groupA-r6-red.log`.
