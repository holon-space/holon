---
id: 2026-07-13-documented-opfs-reset-recipe-silent-page
date: 2026-07-13
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Documented OPFS reset recipe was a silent no-op: page-side removeEntry
  throws NoModificationAllowedError (worker holds sync handles), catch
  swallowed it
source_line: 989
---

## Bug

Documented OPFS reset recipe was a silent no-op: page-side removeEntry
throws NoModificationAllowedError (worker holds sync handles), catch
swallowed it

## Missing piece

no check that reset actually emptied the directory

## Remedy

FIXED (B2): reset now runs inside the worker (which owns the sync handles) —
`unregisterFile` closes them before `removeEntry`; non-NotFoundError
removeEntry failures are re-thrown, not swallowed
