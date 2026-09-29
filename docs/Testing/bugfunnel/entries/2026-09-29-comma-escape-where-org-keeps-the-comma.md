---
id: 2026-09-29-comma-escape-where-org-keeps-the-comma
date: 2026-09-29
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Holon wrote and removed a comma escape inside quote, center, verse and other
  blocks and inside drawers, where org keeps the comma as text; an
  Emacs-authored `,* x` there read as `* x`, and Holon-typed text gained a
  comma Emacs shows.
---

## Bug
Found by the org-faithful group B r9 verifier (`lane-logs/groupB-r9-verify.md`,
A), measured in `emacs -Q --batch`: org removes an escape comma only inside
example, export and src blocks (org-element.el:2623/2703/3152). Holon escaped
every `#+` and headline-shaped line of block text with the same guard, and
unescaped with it, so its own round trip hid the difference. `losses=[]`.
Source lines with indentation (`  ,#+y`) kept their comma, which org removes.

## Root cause
`comma_escape.rs` decided per line with no knowledge of the block or drawer
the line stands in.

## Missing piece
The test oracle was Holon's own reader; no test compared with org's reading
of each context.

## Remedy
The codec reads each line's context (paragraph; example/export/src: org's own
escape at any indentation; any other block, a dynamic block or a drawer: no
escape). Text Holon writes there that org would read otherwise is written as
it is, and the read-back check records the loss. Keyword lines org reads
inside quote/center/special blocks and lists are kept as keyword-line
carriers, as in section text. Paragraph text keeps the D230.a escape pending
a ruling. Pinned by `org_reads_as_emacs_reads.rs` (`a_comma_org_keeps_is_text`,
`a_comma_org_removes_is_removed`, `a_line_org_would_read_otherwise_is_a_loss_not_a_comma`);
measurements in `lane-logs/B10-emacs-contexts.log`.
