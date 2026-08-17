---
id: 2026-07-10-firingkey-included-cdc-integer-created-path
date: 2026-07-10
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  FiringKey included CDC `_rowid` (Integer on Created path, String on Updated
  path) → same day's journal mints path-dependent deterministic ids →
  cross-replica at-most-once broken for mixed boot/rollover firings (found by
  capstone agent's static probe, not a test)
source_line: 845
---

## Bug

FiringKey included CDC `_rowid` (Integer on Created path, String on Updated
path) → same day's journal mints path-dependent deterministic ids →
cross-replica at-most-once broken for mixed boot/rollover firings (found by
capstone agent's static probe, not a test)

## Missing piece

neither the two-replica convergence PBT nor the keystone can generate two
replicas firing the same key via *different* CDC paths

## Remedy

FIXED (`FiringKey::from_row` excludes `_`-prefixed internal columns) +
`firing_key_excludes_internal_columns` pins it; mixed-path replica
generation in convergence PBT still open
