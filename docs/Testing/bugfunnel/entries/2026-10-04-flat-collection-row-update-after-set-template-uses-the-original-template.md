---
id: 2026-10-04-flat-collection-row-update-after-set-template-uses-the-original-template
date: 2026-10-04
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  After a view-mode switch (`set_template`), a row that CDC updates, inserts or rebuilds in a
  flat collection renders with the original item template, so
  that row shows the old view mode among rows that show the new one.
---

## Bug
Found by code reading in the non-finite float lane (round 9), while the flat
template driver was changed. A verifier reproduced it with a probe test (not
kept, no measurement in the running app): a flat collection with `text(x)`, a
row with x=1.0, `set_template(text(x*10))` (the row correctly becomes "10"),
then a CDC `Change::Updated` with x=2.0 renders "2" instead of "20". Expected: after `set_template(new)`, every row of the
collection, including a row that a later CDC update replaces, renders with
`new`. Observed in the code: the row renders with the template the collection
was built with.

## Root cause
`ReactiveView::set_template` sets `template_mutable`
(crates/holon-frontend/src/reactive_view.rs, the `Collection` arm near line 987).
Only the template driver of the flat driver reads `template_mutable` and
re-interprets the live items. The other two paths of the same driver read the
`item_template` argument, which is the template at construction time:
- `full_rebuild` (near line 2021): `let tmpl = item_template.clone();`
- `data_driver` (near line 2069): `let tmpl = item_template.clone();`, used by
  `UpdateAt`, `InsertAt` and `Push`.

`full_rebuild` is also started by the space driver (near line 2158), the
profile driver (near line 2176) and the focus driver (near line 2200). So a
row that CDC replaces, and every row after a sort-key rebuild, a viewport
change, a profile edit or a focus change, goes back to the original template. The tree driver and the grouped driver were not checked.

## Missing piece
No test and no keystone transition does a view-mode switch and then a row
update on the same collection. The keystone does not switch view modes, and the
unit tests of the template driver switch the template without a later CDC event.

## Remedy
Open. The fix is to read the current value of `template_mutable` in
`full_rebuild` and in the `data_driver` instead of the captured
`item_template`. First write a red test: build a flat collection, call
`set_template`, send a CDC update for one row, and assert that the row's node
equals the node that the new template gives. Then check the tree and grouped
drivers for the same capture.
