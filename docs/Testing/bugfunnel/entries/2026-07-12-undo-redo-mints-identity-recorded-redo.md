---
id: 2026-07-12-undo-redo-mints-identity-recorded-redo
date: 2026-07-12
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Undo/redo of `create` re-mints identity: the recorded redo op carries NO
  `id` param (params = content+parent only), so redo after undo resurrects the
  block under a NEW uuid (`4f612fa4`→`24559cf8`) — any block ref/link/junction
  row targeting the old id dangles
source_line: 900
---

## Bug

Undo/redo of `create` re-mints identity: the recorded redo op carries NO
`id` param (params = content+parent only), so redo after undo resurrects the
block under a NEW uuid (`4f612fa4`→`24559cf8`) — any block ref/link/junction
row targeting the old id dangles

## Missing piece

undo entry recorder must persist the minted id in the redo params; no
keystone undo/redo alphabet to catch it

## Remedy

OPEN
