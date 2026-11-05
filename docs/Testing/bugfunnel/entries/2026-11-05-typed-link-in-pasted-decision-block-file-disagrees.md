---
id: 2026-11-05-typed-link-in-pasted-decision-block-file-disagrees
date: 2026-11-05
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  After a decision block is pasted as a copy and `[[..]]` is typed into it, its
  owner's org file holds the brackets while the oracle expects the bare text.
---

## Bug
Found by a keystone replay (`general_e2e_composed_pbt`, seed
4497092337857983733) during the required-clock lane, first seen on the run of
the touched crates after the SqlOperationProvider clock migration. The tail of
the shrunk case is `PasteBlockCopy kd-1` (decision-page → another page),
`NavigateFocus decision-page`, `FocusEditableText kd-1`, `TypeChars "[[aa]]"`.
`inv-copies-stay-on-disk` fails: the owner's file decision-page.org holds
`* ? Which store holds the decision?[[ig]] :decision:` while the oracle expects
the text `Which store holds the decision?ig`.

## Root cause
Not measured. The red is older than the clock work: the same seed is red with
the same signature on the tree that holds only the shape-op id resolution
(`Summary [ 812.511s] 1 test run: 0 passed, 1 failed`, lane log
`rev1-tree-keystone-seed-4497.log:616`), and on the tree with the required
clock key (`rev4-probe-keystone-seed.log`). Open question: does the oracle
strip `[[..]]` that the product keeps (then FALSE-ALARM), or does the product
write raw brackets where it should store a link (then a product defect)?

## Missing piece
Classification is provisional: the keystone caught it, so the gap is set by
whichever side turns out wrong. ORACLE if the oracle's text model of a typed
link is wrong; a product defect otherwise.

## Remedy
Open. Needs a probe at the TypeChars apply on a decision block (oracle text vs
the `set_field` content) and a known-red row decision.
