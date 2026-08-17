---
id: 2026-07-22-keystone-fresh-seed-draws-hit-unremapped
date: 2026-07-22
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  Keystone: fresh-seed draws hit an unremapped synthetic `block:ref-doc-0`
  peer-doc — `inv-viewmodel-entity-ids-subset-of-data` phantom entity +
  `inv-blocks-match-ref/org` divergence (shrink:
  AddPeer/CreateDoc/PeerEdit/CreateBlockUnderFocus/Navigate/UndoLastMutation
  on a peer doc). Discovered by the row-28 verifier's independent third seed;
  reproduced parser-independent (fix reverted). Was MASKED for weeks by the
  row-28 sanctioned red absorbing all keystone-red attention.
source_line: 800
---

## Bug

Keystone: fresh-seed draws hit an unremapped synthetic `block:ref-doc-0`
peer-doc — `inv-viewmodel-entity-ids-subset-of-data` phantom entity +
`inv-blocks-match-ref/org` divergence (shrink:
AddPeer/CreateDoc/PeerEdit/CreateBlockUnderFocus/Navigate/UndoLastMutation
on a peer doc). Discovered by the row-28 verifier's independent third seed;
reproduced parser-independent (fix reverted). Was MASKED for weeks by the
row-28 sanctioned red absorbing all keystone-red attention.

## Missing piece

The harness's reference-doc placeholder remap (reference_state.rs:798,
action_actor_state.rs:52) does not survive this op sequence; no invariant
pinned the remap totality. Remedy: make the ref-doc id remap total across
peer-doc creation + undo, then the keystone is green with ZERO known reds.

## Remedy

OPEN 2026-07-22 — interim: this exact signature is the ONLY tolerated
keystone red (harness comment); any other red = regression.
