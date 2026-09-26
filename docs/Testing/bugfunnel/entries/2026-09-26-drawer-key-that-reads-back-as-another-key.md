---
id: 2026-09-26-drawer-key-that-reads-back-as-another-key
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A property key ending in `+` or spelled `id`/`Id`/`iD` was accepted and
  written, but read back as another key (`note+` as `note`) or not at all.
---

## Bug
Found by the adversarial verifier of org-faithful group A
(`lane-logs/groupA-verify.md`, defect 2): `DrawerKey::parse` admitted `note+`,
`id` and `Id`, and `docs/Reference/ORG_SYNTAX.md` called the key rule complete.

## Root cause
orgize's `node_property_node` (`src/syntax/drawer.rs` in the holon orgize
fork) splits a trailing `+` off the key as org's append syntax.
`extract_properties` and `extract_or_generate_id`
(`crates/holon-org-format/src/parser.rs`) treat every ASCII-case spelling of
`ID` as the identity and drop it from the properties.

## Missing piece
The codec PBT key generator produced only plain lower-case keys.

## Remedy
`DrawerKey` refuses a trailing `+` and every spelling of `ID`; the engine
refuses such a key (`drawer_key_write_boundary.rs`), the renderer leaves it
out with a warning, and the codec PBT generates these keys.
ORG_SYNTAX.md states the rule. The live vault has no such key (0 files).
