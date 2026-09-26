---
id: 2026-09-26-padded-or-empty-property-value-changes-on-org-round-trip
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A property value with space at the start or end came back trimmed from the
  org file, and an empty value on a headline drawer came back as no key at all.
---

## Bug
Found by the org-faithfulness planner's probe
(`crates/holon-org-format/tests/org_faithful_probe.rs`, lane
`org-drawer-faithful`): `note = " padded "` read back as `"padded"`, and
`note = ""` read back as a missing key. Neither loss was visible.

## Root cause
The renderer wrote `:note:  padded ` and `:note: `. The parser trims every
drawer value (`extract_properties`, `crates/holon-org-format/src/parser.rs`),
and orgize's `PropertyDrawer::iter()` yields no pair for an empty value.

## Missing piece
The keystone's drawer-value generator drew only `[a-zA-Z0-9]{1,10}`, so no
case held surrounding space or an empty value.

## Remedy
The org value codec (`crates/holon-org-format/src/drawer.rs`, rule in
`docs/Reference/ORG_SYNTAX.md`) writes such a value as a JSON string literal
(`" padded "`, `""`) and reads it back byte-equal. The file-level drawer keeps
an empty value raw (`:KEY: `), because its own reader keeps it. Pinned by
`crates/holon-org-format/tests/drawer_value_codec_pbt.rs` and the keystone's
`drawer_value_strategy` plus the hand-authored cases
`external-drawer-values-round-trip-{loro,sqlonly}-arm`. The org capability
profile now declares `property_values.empty_string: representable`
(certified by `crates/holon-org-format/tests/profile_certification.rs`).
