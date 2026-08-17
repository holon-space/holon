---
id: 2026-07-13-app-hard-errored-showed-empty-root
date: 2026-07-13
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  App hard-errored (or showed empty "No root layout") instead of recovering
  when root-layout absent; no in-app recovery path
source_line: 987
---

## Bug

App hard-errored (or showed empty "No root layout") instead of recovering
when root-layout absent; no in-app recovery path

## Missing piece

keystone cannot generate a "root-layout absent at boot" state to assert
graceful recovery

## Remedy

FIXED (B2): new `BootState::NoRootLayout` renders a styled recovery card
with a "Reset local data" action. Reset runs WORKER-side
(`engine_reset_storage` napi export → drop engine so Turso releases OPFS
handles; worker-entry then `unregisterFile`+`removeEntry` db/wal/shm) then
reloads — closing F4's page-side removeEntry NoModificationAllowedError.
Verified: clicked Reset on the pre-existing corrupt vault → cleared +
re-seeded fresh
