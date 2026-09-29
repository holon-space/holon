---
id: 2026-09-29-drawer-value-trailing-space-dropped
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A headline drawer value's authored spacing was lost on write-back, and a
  whitespace-only value lost its whole property line, with no loss reported.
---

## Bug
Found by the org-faithful group B r7 vault gate (`lane-logs/B7-vault.log`, a
hook-written agent status page) and widened by the r6–r7 verifier
(`lane-logs/groupB-r67-verify.md`). Measured, all `losses=[]`, pre-existing:
`:NOTE: two ` → `:NOTE: two`; `:NOTE:  lead` → `:NOTE: lead`; `:NOTE: a\tb\t`
→ `:NOTE: a\tb`; `:NOTE: ` → the whole line gone.

## Root cause
The headline drawer was read through orgize's `PropertyDrawer::iter()`, which
trims each value and drops a key whose value is only whitespace; nothing kept
the authored bytes.

## Missing piece
No generator writes a drawer value with authored spacing into a file.

## Remedy
`drawer_entries` (`crates/holon-org-format/src/parser.rs`) reads the headline
drawer line by line, so a whitespace-only value is a property with an empty
value. The authored bytes of every value the renderer would write differently
are kept in `_drawer_raw`, and `format_properties_drawer`
(`crates/holon-org-format/src/models.rs`) writes them back while they still
read as the block's value. Pinned by `org_text_reads_back_as_written.rs`
(`a_drawer_value_keeps_its_authored_spacing`) and holon-app
`org_store_org_round_trip.rs` (`drawer_value_spacing_survives_the_store`,
both write legs). A value-less `:NOTE:` (no space) is a separate open defect,
`2026-09-29-value-less-headline-drawer-key-voids-the-drawer`.
