---
id: 2026-09-30-carrier-named-user-property-blocks-page-write-back
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A user property named `file_properties` on a page made its org render fail,
  which blocked write-back of the whole page.
---

## Bug
Found by the org-faithful group B r11 verifier
(`lane-logs/groupB-r11-verify.md`, D2v): `file_properties = {"ID":"evil"}`
on a doc root made `render_document` refuse the page.

## Root cause
The doc-root carriers `file_properties` and `file_id_keyword` had no `_`
prefix, so a user property of the same name was read as the carrier.

## Missing piece
No generated operation sets a property whose name is a carrier's.

## Remedy
Both carriers are `_`-prefixed (`_file_properties`, `_file_id_keyword`);
`_file_id_keyword` is in `org_props::PARSER_CARRIERS`. `_file_properties`
stays writable by an engine write, which checks its keys and id (the page's
drawer is user-editable, `a_file_drawer_key_ending_in_plus_is_written`). An
authored drawer key that starts with `_` is stored behind `\` (`AuthoredKey`),
so no file writes either carrier. Pinned by
`a_user_property_named_like_a_carrier_is_user_data`.
