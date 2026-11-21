---
id: 2026-11-21-light-paths-gate-crashes-on-card-without-pr
date: 2026-11-21
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The light-paths gate raised KeyError on a light card with no linked PR and judged heads of other repos against
  holon's test selection, so every fork landing stayed stuck in build.
---

## Bug

`scripts/light-paths-gate.py` read `environ["FLOW_PR_HEADS"]`, which flow sets only for a card with linked PRs. A light
card with none (a fix landed in another repo) crashed the gate with `KeyError`. A head of another repo was judged as
if its paths were holon paths. Found by the steward's dry run of the move into verify on a fork landing.

## Root cause

The gate assumed every light card has a holon PR. Its tests always set `FLOW_PR_HEADS` and always used one repo name.

## Missing piece

No test ran the gate with the variable absent, and none used a head of a repo other than the one under test.

## Remedy

An absent `FLOW_PR_HEADS` passes with a named line; a present but empty value is still refused. Only heads of
`HOLON_REPO` are judged, others are named as skipped and never fetched. `scripts/test-light-paths-gate.py` gained P6 with
fixed cases for the absent variable and the foreign-head shapes, asserting that `gh` saw no call for a foreign repo.
