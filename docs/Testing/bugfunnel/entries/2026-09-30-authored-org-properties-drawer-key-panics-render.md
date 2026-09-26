---
id: 2026-09-30-authored-org-properties-drawer-key-panics-render
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A headline drawer line under the key `org_properties` made the org renderer
  panic: the parser stored it under the drawer carrier's own key.
---

## Bug
Found by the decision Inc 6 rebase onto the org-faithful group B work, when an
Inc 6 dense_patch test (`frontends/mcp/tests/dense_patch_exact.rs`) wrote a
drawer line `:org_properties: ...` and the render panicked
(`lane-logs/inc6-rebase-report.md`). Reproduced on main's
`crates/holon-org-format/src/drawer.rs` by
`an_authored_org_properties_key_reads_back_as_written`: panic at
`crates/holon-org-format/src/models.rs:325`, "malformed org_properties JSON
\"x\"" (`lane-logs/inc6rb2-red-authoredkey.log`).

## Root cause
`AuthoredKey` (`crates/holon-org-format/src/drawer.rs`) escapes an authored
key that starts with `_` or `\`, the keys the property bag keeps for Holon's
own. `org_properties` is also Holon's own (the drawer carrier,
`org_props::ORG_PROPERTIES`), but was not escaped, so the authored value
landed in the carrier and the renderer parsed it as the drawer's JSON.

## Missing piece
No drawer-key generator or round-trip test names a key that collides with a
Holon property-bag key other than the `_` prefix.

## Remedy
`AuthoredKey` treats `org_properties` as reserved and stores the authored key
behind a `\`. Pinned by `an_authored_org_properties_key_reads_back_as_written`
(`crates/holon-org-format/tests/org_text_reads_back_as_written.rs`).
