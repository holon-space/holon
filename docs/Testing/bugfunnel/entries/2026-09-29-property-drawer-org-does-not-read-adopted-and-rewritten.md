---
id: 2026-09-29-property-drawer-org-does-not-read-adopted-and-rewritten
date: 2026-09-29
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A PROPERTIES drawer org does not read (a key with a space, a blank or text
  line inside) gave Holon an id and properties org does not see, and every
  unedited headline drawer was rewritten on write-back: extra lines deleted,
  spacing, key case and indentation normalized, `losses=[]`.
---

## Bug
Found by the org-faithful group B r9 verifier (`lane-logs/groupB-r9-verify.md`,
B). `:ID: aaa` + `:MY KEY: v`, a blank line, or a second `:ID:` line: the
extra line was deleted on the first write-back of an unedited file. `:ID:   aaa  `,
`:id:`, an indented drawer, `:END:  ` were rewritten.

## Root cause
The r9 lenient drawer reader accepted any PROPERTIES drawer at the section
start; the renderer always wrote its canonical drawer.

## Missing piece
No test compared Holon's drawer reading with org's (org-property-drawer-re,
org-entry-get), and no test fed an unedited non-canonical drawer.

## Remedy
`read_property_drawer` (`parser.rs`) reads a drawer as org does: every line
`:KEY:` then a blank or the line's end, key with no whitespace (colons kept,
`:a:b: v` is key `a:b`). A drawer org does not read gives no properties; its
first `:ID:` line still names the block, so Holon does not re-identify it.
The authored drawer is kept in the `_drawer_text` carrier and written back
while the block's id and drawer values are unchanged; a drawer org does not
read (also one after blank lines), or one with two `:ID:` lines (Holon takes
the last, as org-entry-get and org-id links do), is disclosed as a loss on
every write. An edited block gets a canonical drawer; a drawer org does not
read then stays below it as text, and each dropped `:ID:` line is named. Measured shapes: `lane-logs/B10-emacs-drawers.log`.
Pinned by `org_reads_as_emacs_reads.rs`.
