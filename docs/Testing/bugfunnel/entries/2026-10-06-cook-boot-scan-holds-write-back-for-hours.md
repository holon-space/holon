---
id: 2026-10-06-cook-boot-scan-holds-write-back-for-hours
date: 2026-10-06
gap: ENVIRONMENT
secondary: ORACLE
status: PARTIAL
summary: >-
  In the dev profile the boot scan parsed each refused `.cook` recipe in about
  1.05 s, so Martin's 4352 recipes held the boot scan and the org write-back
  for about 75 minutes.
---

## Bug
Found by profiling Martin's live instance (dogfood build, dev profile), not by
a test. Org write-back stayed blocked for over an hour after boot. Lane:
cook-scan Fix 1.

## Root cause
wasmi runs the `.cook` plugin guest. In the dev profile wasmi compiles at
opt-level 0 with debug assertions, so one refused recipe costs about 1.05 s of
CPU (1.03-1.14 s measured); 4352 recipes project to about 75 min
(`vault_scan_latency.rs` prints 4485 s). A recipe is refused only after its
parse (`crates/holon-plugin-host/src/adapter.rs`, `run` before
`parse_local_id`), so every refused file pays the full cost. With wasmi at
opt-level 3 and no debug assertions a refused recipe costs about 9 ms.

## Missing piece
No test ran the plugin parse at vault scale in the dogfood profile: the
keystone PBT uses a few recipes and CI runs release-like timings. Open rungs:
- No wall-time bound on the real scan (Test B is not written), so mutex wait,
  file I/O and scheduler delay are unbounded by any test.
- The Test A fixture parses an in-memory string under a fake root and does no
  file I/O.

## Remedy
`Cargo.toml`: `[profile.dev.package.wasmi*]` at opt-level 3 without debug
assertions. Closing rung: Test A,
`refusing_a_recipe_stays_within_the_scan_budget_in_every_build_profile` in
`crates/holon-plugin-host/tests/vault_scan_latency.rs`, a 20 ms thread-CPU
budget per refused recipe, red at 1.03 s with the profile override removed.
Status PARTIAL until Test B bounds the real scan in wall time.
