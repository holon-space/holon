---
id: 2026-09-30-a-refused-undone-deletion-record-is-copied-aside-on-every-boot
date: 2026-09-30
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A vault whose undone-deletions file holds a refused record, and where nothing else changes, gets a new `undone-deletions.json.unreadable-<ms>` copy on every boot.
---

## Bug
Found by reading by the D229 round-8 verifier (lane d229-move), Residual 2 note. Report: `lane-logs/d229r8v-verify.md`.

## Root cause
`keep_refused_undone_deletions` copies the file aside at load; the refused record leaves the file only at the next write, which a quiet vault never makes.

## Missing piece
No test boots twice over a refused record.

## Remedy
Open (low): write the file without the refused records at load, after the copy aside.
