---
id: 2026-10-01-undeclared-keyword-check-reads-a-missing-root-as-empty
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `refuse_undeclared_keywords` read a root block missing from the store as an
  empty subtree (`.unwrap_or_default()`), so it refused nothing instead of
  failing loud. Fixed; no test reaches the branch, so the fix is unverified.
---

## Bug
Found by code reading in the Inc 6 round-3 verifier
(`lane-logs/inc6rb3v-verify.md`, finding D), lane decision Inc 6. No run
showed a wrong result.

## Root cause
`refuse_undeclared_keywords` (`crates/holon/src/api/operation_engine.rs`)
read `subtree(root)` a second time after its caller read the root, and turned
`None` into an empty subtree.

## Missing piece
No test removes the root between the caller's read and this read. Every
caller reads the root first, so no test can reach the branch; this entry has
no red log.

## Remedy
`.ok_or_else(|| anyhow!("{op}: block {root} is not in the store"))?`. The
`None` stays a runtime outcome, not a type-level impossibility: `subtree()`
reads mutable store state, so a concurrent delete can remove the root between
the two reads. Status FIXED is by code reading only (unverified by a test).
