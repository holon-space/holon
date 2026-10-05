---
id: 2026-10-05-cook-boot-scan-takes-hours-in-the-dev-build
date: 2026-10-05
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  In the dev build each `.cook` file cost about 1.05-1.3 s of boot scan, so
  Martin's 4352 recipes held the boot scan and the org write-back for well over
  an hour; Loro sync and MCP sync started degraded after 600 s and SQL reads
  stalled.
---

## Bug
Found by Martin dogfooding 2026-10-05 (dev build, `target/debug/holon-gpui`),
then profiled on his live instance 2026-10-06 (lanes cook-invest and cook-scan
Fix 1). Live log, first 27 min: the org files finished in 1.5 min, then 604 of
4352 recipe files were scanned. `boot_file` per recipe: median 1269 ms for a
refused file, 1847 ms for an accepted one (p95 about 4-5 s). Org write-back
stayed blocked for over an hour. Side effects in the same log:
`sync gate never opened` (3x, MCP) and `boot gate never opened` (1x, Loro)
after 600 s; `SQL actor stuck` on `SELECT id, parent_id FROM block` for up to
15 s.

## Root cause
wasmi, an interpreter, runs the `.cook` plugin guest
(`crates/holon-plugin-host/src/host.rs`). In the dev profile wasmi compiled at
opt-level 0 with debug assertions, so one refused recipe cost about 1.05 s of
CPU (1.03-1.14 s measured); 4352 recipes project to about 75 min
(`vault_scan_latency.rs` printed 4485 s). A recipe was refused only after the
guest ran, so every refused file paid the full cost.

Every boot also re-parsed every refused recipe, twice: a probe booted a vault
holding only `Grüne Soße.cook` four times, `guest_parses()` grew by 2 on each
boot, and the `file` table held no row for the recipe. Without a row there is
no stored hash, so the cold-boot skip in
`crates/holon-filesystem/src/file_sync_controller.rs` cannot apply.

Why opt-level 2/3 overflowed the stack with debug assertions: wasmi 2.0's
`build.rs` turns on tail-call dispatch (`wasmi_use_tail_calls`) at opt-level
2, 3, s or z, and relies on LLVM's sibling-call optimization. With
`debug-assertions` on in wasmi and its sub-crates that optimization does not
happen, so every executed wasm instruction keeps a stack frame. A deeper stack
is no fix: with `RUST_MIN_STACK` = 1 GiB the 200-step recipe still overflows.

## Missing piece
No test ran the plugin parse at vault scale in the dogfood profile: the
keystone PBT uses a few recipes, and `vault_scan_latency` measured scale only
as an `#[ignore]`d release run. Open rungs:
- No wall-time bound on the real scan, so mutex wait, file I/O and scheduler
  delay are unbounded by any test.
- The Test A fixture parses an in-memory string under a fake root and does no
  file I/O.

## Remedy
`Cargo.toml`: `[profile.dev.package.wasmi*]` at opt-level 3 without debug
assertions. Closing rung: Test A,
`parsing_a_recipe_stays_within_the_scan_budget_in_every_build_profile` in
`crates/holon-plugin-host/tests/vault_scan_latency.rs`, a 20 ms thread-CPU
budget per parsed recipe, red at 1.03 s with the profile override removed.

Most refusals are gone: format plugins emit typed ids that the host
percent-encodes (`2026-10-05-cook-file-name-with-space-or-umlaut-is-refused`),
so a file name with a space or an umlaut ingests. The adapter parses the
source path into its `LocalId` before the guest runs, so a path that still
names no document (an empty segment) is refused without a guest run. Closing
rung: `a_recipe_whose_path_cannot_be_stored_is_refused_before_the_guest_runs`
in the same file asserts that no guest run happens for such a recipe.

D108.a removes the recipes from the boot: the boot scan ingests the writable
files, seeds the default layout and starts the watch loop (and with it the
write-back); the read-only plugin files are a backlog that the watch loop
ingests last (`crates/holon-orgmode/src/di.rs`, `VaultBacklog`,
`VaultBacklogDrained`). No boot step before the ready signal reads a recipe.
Closing rungs:
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
