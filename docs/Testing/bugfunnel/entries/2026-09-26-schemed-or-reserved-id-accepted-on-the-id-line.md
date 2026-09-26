---
id: 2026-09-26-schemed-or-reserved-id-accepted-on-the-id-line
date: 2026-09-26
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  The engine accepted an id that is not a bare block id (`doc:x`,
  `sentinel:no_parent`, `:END:`, `#+ID:`, `a:b`, `kid+`, `*kid`, 5000
  characters) and wrote it to the `:ID:` line, so the headline read back as a
  foreign entity or as the root-parent sentinel.
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
`DrawerId` is a bare block id: at most 255 characters from the RFC 3986
unreserved set, in parts joined by `:` or `::`, naming no URI scheme, and
reading back as `block:<id>`. The engine refuses any other id on the `ID`
property, the `properties` bag, `org_properties`, `file_properties` and a
`create`'s `block:` id (`an_id_that_is_not_a_bare_block_id_is_refused_on_every_route`,
red `lane-logs/groupA-r3-red-engine.log`). The vault and the seeds hold no id
the rule refuses (`lane-logs/groupA-r3-write-rules-census.log`).

Open: the parser still accepts any authored `:ID:` that forms a URI, so text
an external editor wrote (`:ID: doc:x`) is not refused on read.
