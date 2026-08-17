---
id: 2026-07-20-crash-mid-boot-wedges-all-subsequent
date: 2026-07-20
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Crash mid-boot WEDGES all subsequent boots: after the host-GPU wgpu
  device-loss killed the app mid-boot (row above), every later launch failed
  loud with `BOOT FAILED … HolonFrontendModule phase=configure: consolidator
  marker /data/user/0/space.holon.gpui/files/.holon/consolidator is missing a
  'consolidator = ' line` → SIGABRT, with NO recovery path — only `pm clear`
  (full data wipe) unwedged it. The marker file was created but left without
  its `consolidator = ` line by the dying process, i.e. the marker write is
  not crash-atomic (no temp-file+rename), and boot treats a malformed marker
  as fatal instead of quarantine-and-reinit. Fail-loud is right;
  unrecoverable-without-data-wipe is not — on a real device this wedge would
  take user data hostage after any mid-boot crash.
source_line: 1031
---

## Bug

Crash mid-boot WEDGES all subsequent boots: after the host-GPU wgpu
device-loss killed the app mid-boot (row above), every later launch failed
loud with `BOOT FAILED … HolonFrontendModule phase=configure: consolidator
marker /data/user/0/space.holon.gpui/files/.holon/consolidator is missing a
'consolidator = ' line` → SIGABRT, with NO recovery path — only `pm clear`
(full data wipe) unwedged it. The marker file was created but left without
its `consolidator = ` line by the dying process, i.e. the marker write is
not crash-atomic (no temp-file+rename), and boot treats a malformed marker
as fatal instead of quarantine-and-reinit. Fail-loud is right;
unrecoverable-without-data-wipe is not — on a real device this wedge would
take user data hostage after any mid-boot crash.

## Missing piece

No transition in the keystone catalog kills the SUT mid-boot and relaunches
it over the surviving data dir (crash-durability/restart rung absent), so
torn on-disk state from an interrupted boot is ungeneratable. Secondary
ENVIRONMENT: marker lives in the mobile data-dir wiring. Remedies: (a) write
marker via temp-file+rename (atomic); (b) boot handles present-but-malformed
marker as disclosed reinit, not abort; (c) keystone kill-mid-boot→relaunch
transition would have caught both.

## Remedy

OPEN 2026-07-20 (agent exploration, same session as row 233; recovered via
`pm clear` on the emulator)
