---
id: 2026-09-30-an-adopted-copy-ends-the-undone-deletion-record-before-its-change-is-durable
date: 2026-09-30
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A crash after an adoption ended an undone deletion, but before the deletion reached the store and the files, brings the deleted child back silently at the next boot.
---

## Bug
Found by the D229 round-7 verifier (lane d229-move), probes VP-B/VP-B3 in `lane-logs/d229r7v-probes*.log`, through the lane's crash seam. Report: `lane-logs/d229r7v-verify.md`, Defect 1.

## Root cause
The write-ahead persist ran after the adoption had already ended the records, so the record left the vault before its change was durable.

## Missing piece
The keystone does not crash mid-ingest; the round-7 crash tests covered only the put-back arm.

## Remedy
Fixed in round 8: an ended record stays in the vault (`ending_undone`) while one of its files still holds the block (R12.1); seven `a_standing_deletion_survives_a_crash_at` tests.
