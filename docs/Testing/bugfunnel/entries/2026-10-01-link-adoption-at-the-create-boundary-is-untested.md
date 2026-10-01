---
id: 2026-10-01-link-adoption-at-the-create-boundary-is-untested
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Adoption of `[[links]]` in content that rides a block create (the cell-leg
  `create_entity_sync`) is not pinned by any test, so a create carrying content
  can store text with no link marks or junction rows.
---

## Bug
Found by reading, not reproduced: read-only spike SP4 section 5, item 3
(`scoped-net/sp4-newborn-census.md` in the session scratchpad). Today the first
typed content goes through an ordinary user `set_field`, which adopts links; the
empty create carries none.

## Root cause
By reading, not reproduced. `create_entity_sync`
(`crates/holon-loro/src/block_cell_registry.rs:498`) calls
`create_block_with_properties_sync` with the given content and never runs the
adoption step that the edit arm runs. The existing pin
`first_content_typed_into_an_empty_born_block_still_adopts_its_links`
(`crates/holon/tests/live_edit_link_marks.rs:290`) covers the empty birth plus
a `set_field`, and says the birth "carries no content for adoption to have
missed". Keystone row 524 covers typed text after the birth. Nothing covers a
create that carries content.

## Missing piece
No test and no keystone draw creates a block with `[[link]]` content in one
firing and asserts marks and junction rows.

## Remedy
OPEN. Add a test of create-with-content adoption on the cell leg and a keystone
draw, before any change makes the first character ride the create.
