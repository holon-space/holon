---
id: 2026-07-31-reference-model-diverges-sut-editor-authored
date: 2026-07-31
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The reference model diverges from the SUT on `inv-editor-text/mirror` for
  ANY editor-authored explicit-label wiki link, blocking deterministic
  hand-authored replay of link cases: reference keeps raw markup (`see [[Some
  Page][lbl]]`) while the SUT's `MutableText` holds the stripped label (`see
  lbl`). The reference does not run the org lens on `CreateBlockUnderFocus`
  content. Independent of link classification — probed with `[[Some
  Page][lbl]]`, whose classification the F1a change does not touch, and the
  divergence is identical. Found while attempting to add hand-authored
  regression cases for entity links.
source_line: 1130
---

## Bug

The reference model diverges from the SUT on `inv-editor-text/mirror` for
ANY editor-authored explicit-label wiki link, blocking deterministic
hand-authored replay of link cases: reference keeps raw markup (`see [[Some
Page][lbl]]`) while the SUT's `MutableText` holds the stripped label (`see
lbl`). The reference does not run the org lens on `CreateBlockUnderFocus`
content. Independent of link classification — probed with `[[Some
Page][lbl]]`, whose classification the F1a change does not touch, and the
divergence is identical. Found while attempting to add hand-authored
regression cases for entity links.

## Root cause

the reference model diverges from the SUT on `inv-editor-text/mirror` for
ANY editor-authored explicit-label wiki link — reference keeps raw markup
(`see [[Some Page][lbl]]`), SUT `MutableText` holds the stripped label (`see
lbl`), because the reference does not run the org lens on
`CreateBlockUnderFocus` content. Independent of link classification: probed
with `[[Some Page][lbl]]`, whose classification F1a does not touch, and the
divergence is identical. Blocks deterministic hand-authored replay of any
link case. Reference-model gap, not a product defect — the SUT is correct.)

## Missing piece

Reference-model gap, not a product defect: the SUT behaviour is correct and
the reference is the one that is wrong. Consequence is that
`hand-authored-regressions/keystone.jsonl` cannot express any
editor-authored link case, entity or page. Missing piece = applying the org
lens to content in the reference's create path.

## Remedy

OPEN 2026-07-31 — diagnosed, NOT fixed; out of scope for the F1a lane.
Entity-link coverage was instead placed at the org-ingest and
`set_field(content)` boundaries, which the reference models correctly.
