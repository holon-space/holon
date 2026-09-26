---
id: 2026-09-27-id-rule-refuses-ids-the-file-carries
date: 2026-09-27
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The `DrawerId` rule of org-faithful group A round 4 admitted only RFC 3986
  unreserved characters, so it refused whole files for ids that round-trip
  byte-stably (`a/b`, `a%20b`, `a=b`) — including the `<relpath>::b::<n>`
  ids the Markdown adapters mint for a file in a subdirectory.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 4
(`lane-logs/groupA-r4-verify.md`, defect 2). Not landed. The round changed
the mirroring test fixture in `crates/holon-orgmode/tests/ingest_contract.rs`
to fit the rule instead of the rule to fit production
(`crates/holon-markdown/src/obsidian.rs`, `logseq.rs`).

## Root cause
The rule was a character whitelist, stricter than its contract ("an id the
org file carries and reads back as the same id, with no scheme").

## Missing piece
The id tests listed hand-picked ids; none drew from the ids production mints.

## Remedy
`DrawerId::parse` (`crates/holon-org-format/src/drawer.rs`) states the
contract: non-empty, at most 255 bytes, forms `block:<id>` with the same id,
names no scheme, and does not start with `:`. The property
`every_minted_id_is_carried_as_itself`
(`crates/holon-org-format/tests/org_id_contract.rs`) draws UUIDs, Markdown
ids with subdirectories, `::src::` ids, Loro ids and slugs, and round-trips
each on every carrier (red `lane-logs/groupA-r5-red.log`, minimal input
`%3D.md::b::0`). The fixture is back to production-shaped ids.
