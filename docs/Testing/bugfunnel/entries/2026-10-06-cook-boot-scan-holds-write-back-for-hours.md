---
id: 2026-10-06-cook-boot-scan-holds-write-back-for-hours
date: 2026-10-06
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
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
`parsing_a_recipe_stays_within_the_scan_budget_in_every_build_profile` in
`crates/holon-plugin-host/tests/vault_scan_latency.rs`, a 20 ms thread-CPU
budget per parsed recipe, red at 1.03 s with the profile override removed.

The recipe scope declares `id_from: source_path`
(`crates/holon-plugin-host/plugins/cooklang.yaml`), so the adapter refuses a
recipe whose path cannot be stored before the guest runs: a refused recipe
costs 4-15 µs of CPU instead of 11.3 ms in the test profile. Closing rung:
`a_recipe_whose_path_cannot_be_stored_is_refused_before_the_guest_runs` in the
same file asserts that no guest run happens for such a recipe.

D108.a removes the recipes from the boot: the boot scan ingests the writable
files, seeds the default layout and starts the watch loop (and with it the
write-back); the read-only plugin files are a backlog that the watch loop
ingests last (`crates/holon-orgmode/src/di.rs`, `VaultBacklog`,
`VaultBacklogDrained`). No boot step before the ready signal reads a recipe.
Closing rungs, which take the place of Test B:
- `the_default_layout_seed_is_answered_while_the_recipe_backlog_is_held` and
  `the_default_layout_seed_does_not_wait_for_the_recipe_backlog` in
  `crates/holon-integration-tests/tests/cook_vault_ingest.rs`. Test profile,
  500 recipes: seed at 1183 ms with 0 recipes in, backlog done at 63 239 ms
  ([g-seed-1.txt](../../fixture-logs-2026-10-08/g-seed-1.txt)). With the old
  boot order both are red: seed at 34 723 ms after all 500 recipes
  ([teeth-1.txt](../../fixture-logs-2026-10-08/teeth-1.txt)).
- Keystone hand-authored case
  `the-org-files-sync-while-the-read-only-backlog-is-held`: a reboot with
  every `.cook` read held must report ready, sync org edits and refuse a
  recipe edit ([ks-fix-3.txt](../../fixture-logs-2026-10-08/ks-fix-3.txt)); red
  with the old boot order ([teeth-1.txt](../../fixture-logs-2026-10-08/teeth-1.txt)).
- `a_recipe_page_waiting_in_the_backlog_gets_no_org_twin` in the same test
  file: no page file is written beside a recipe that waits in the backlog
  ([twin-2.txt](../../fixture-logs-2026-10-08/twin-2.txt)).
