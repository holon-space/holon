---
id: 2026-09-26-schemed-or-reserved-id-accepted-on-the-id-line
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The engine accepted an id that is not a bare block id (`doc:x`,
  `sentinel:no_parent`, `:END:`, `#+ID:`, `a:b`, 5000 characters) and wrote
  it to the `:ID:` line, so the headline read back as a foreign entity, as the
  root-parent sentinel, or as another id.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 2
(`lane-logs/groupA-r2-verify.md`, defect 2): through the real engine, 11 of
13 hostile ids were accepted on `set_field org_properties` and written
verbatim. `:ID: doc:x` parsed to a block whose URI is `doc:x`;
`:ID: sentinel:no_parent` collided with `EntityUri::no_parent()`.
`docs/Reference/ORG_SYNTAX.md` says org files store bare ids.

## Root cause
`DrawerId::parse` (`crates/holon-org-format/src/drawer.rs`) accepted
whatever `EntityUri::try_from_raw` accepts, and that returns an
already-schemed string as is. Nothing bounded the character set or length.

## Missing piece
The id tests used only line breaks and whitespace as hostile ids; a unit test
pinned `block:abc` as valid.

## Remedy
`DrawerId` is a bare block id: non-empty, at most 255 bytes, forming
`block:<id>` with the same id, naming no URI scheme and not starting with `:`
(`kid+` and `*kid` read back as themselves and are accepted). It holds at
every boundary:
- The engine refuses any other id on the `ID` property, the `properties` bag,
  `org_properties` and `file_properties`, and a `create` whose `id` is not
  `block:<DrawerId>` (`doc:x`, `file:x`, `sentinel:no_parent` and bare ids
  included) (`an_id_that_is_not_a_bare_block_id_is_refused_on_every_route`,
  red `lane-logs/groupA-r3-red-engine.log` and
  `lane-logs/groupA-r4-red-engine-create.log`).
- The parser refuses a file whose heading `:ID:` (red
  `lane-logs/groupA-r4-red-parser.log`), source block `:id` or page id
  (`#+ID:` or file-drawer `:ID:`, ruling 2026-09-27; red
  `lane-logs/groupA-r5-red.log`) is not a bare block id.
- The renderer fails on such an id by name instead of writing another one,
  on every carrier (`2026-09-26-renderer-rewrites-an-id-it-cannot-write.md`,
  `2026-09-27-source-block-id-rewritten-on-render.md`).

The vault (all 1072 org files, hidden directories included, walked by
`write_rules_census`) and the seeds hold no id the rule refuses, and none of
its 1072 `#+ID:` lines names a scheme (`lane-logs/groupA-r5-vault.log`,
`lane-logs/groupA-r4-seed-id-census.log`).
