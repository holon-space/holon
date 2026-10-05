---
id: 2026-10-05-journals-ingest-deletes-seeded-journal-rule-on-boot
date: 2026-10-05
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  On a fresh SqlOnly (Turso-only) boot, the org initial-scan ingest of a
  block-less Journals.org can run while seed_default_layout creates the
  children of block:journals, and its delete pass purges them; the journal
  auto-create rule then never fires, or the seed fails with "parent block not
  found: block:journals::auto-create".
---

## Bug
Found by `just keystone-smoke` in the cook-plugin-ext lane (commit 3a50b728),
in two runs that drew the Turso wiring: "[boot journal] auto-create rule did
not fire journal ... within budget" and "seed_default_layout failed ... parent
block not found: block:journals::auto-create" with "FOREIGN KEY constraint
failed". That lane blamed a JSON-text block property (`segments`). The blame
was wrong: its A/B runs did not pin the wiring, and every green A/B run drew
Loro storage while every red run drew Turso.

The keystone caught the defect on the lane base b7a59c41, but only by chance.
On main the keystone cannot reach it (see Missing piece).

## Root cause
Measured with `HOLON_PBT_PIN_WIRING="Turso;;"` (logs in the json-prop-boot
workspace, `lane-logs/`):

- main b2a19917: boot passes (`ks-main-turso.log`).
- b7a59c41 (no `segments`, no lane WIP): red, 2 of 2 runs
  (`ks-A-lanebase-turso.log`, `ks-A3-lanebase-turso-info.log`).
- b7a59c41 with the recipe file named `keystone-recipe.cook` again: green
  (`ks-A2-...`, `ks-A4-...`).

b7a59c41 renames the keystone recipe to `Grüne Keystone-Soße.cook`. That name
sorts before `Journals.org`, so the cook file (about 0.5 s of wasm parse in the
dev build) is ingested first, and the `Journals.org` ingest moves later in
time, into the window where `seed_default_layout` runs.

- `seed_default_layout` (`crates/holon-app/src/seed.rs:105-150`, called from
  `crates/holon-app/src/wiring.rs:715`) creates `block:journals` and its four
  children first (`crates/holon-frontend/src/lib.rs:986-994`). The org initial
  scan runs at the same time.
- The `Journals.org` ingest parses 0 blocks. Its delete pass
  (`crates/holon-filesystem/src/file_sync_controller.rs:6096-6106`, applied at
  `:6227`) deletes every stored child of `block:journals` that the file does
  not hold.
- SQL trace (`ks-A9-sqltrace.log`, `ks-A11-1.log`): each child is present just
  after its seed create. Then one 15-statement transaction runs
  `DELETE FROM block_raw WHERE id = 'block:journals::action::0'`, `::auto-create`,
  `::src::0`, `::render::0`. Its span is
  `org.initial_scan.ingest:org.ingest_file{path=.../Journals.org}`, and the
  seed is still in flight. After the seed loop only `block:journals` is left.
  The rule discovery (`assets/queries/holon_rule_discovery.sql`) then finds no
  `holon_rule` block. If the purge lands between the seed's `auto-create` and
  `action::0` creates, the seed fails on the missing parent instead.

The JSON-text property is not the cause. The lane wasm with `segments` could
not run on main (plugin ABI changed in b7a59c41), but the red reproduces at
b7a59c41 without it.

## Missing piece
The keystone boot vault has a fixed set of file names. The one slow file
(the recipe) sorts after `Journals.org` on main, so the Journals.org ingest
never overlaps the seed. Scan order and ingest timing relative to the seed are
not drawn. Also, no invariant says "programmatically seeded blocks survive the
initial scan": the symptom shows only through the boot-journal wait.

## Test enhancement
- The keystone boot vault holds `Archive.org` (`BOOT_FILLER_FILE`, 120 leaf
  headings, modeled by `seed_boot_filler`). It sorts before `Journals.org` and
  takes about 3–4 s to ingest in the test profile, so the `Journals.org`
  ingest runs inside the seed's window, as in a real vault.
- After the org boot ends, `boot_and_seed_wide` asserts that every
  programmatically seeded `block:journals` block exists
  ("[boot seed] programmatically seeded blocks are missing after boot").
- Hand-authored case `boot-seeded-journal-rule-survives-the-initial-scan`
  (`keystone.jsonl`, Turso pin, ends with a `Reboot`).

Red before the fix, `HOLON_PBT_PIN_WIRING="Turso;;" just keystone-smoke`:
3 of 3 (`lane-logs/sas-red-6.log`, `-7`, `-8` in the json-prop-boot
workspace); with 50 headings it was 3 of 4 (`sas-red-2..5`). Hand-authored
case red 3 of 3 (`sas-ha-red-1..3.log`).

## Remedy
FIXED — Martin's ruling D83.a (candidate 1): the seed runs only after the org
initial scan. The vault files own their blocks; the seed fills in what they
lack; write-back puts the seeded blocks into the file.
- `holon_orgmode::BootSeedGate` (`crates/holon-orgmode/src/di.rs`): the
  controller ingests the vault, reports the scan outcome, and waits for the
  seeder's answer before its boot phase (fileless-page materialization, the
  copy-on-write seed baseline, `finish_boot_seeding`) and before
  `signal_ready`. A failed scan skips the seed, and a failed or skipped seed
  is a boot failure on the ready signal (the degraded-mode banner).
- `crates/holon-app/src/wiring.rs`: the gate is registered with the org
  module; the session factory's seed + seed-layout flush moved into the
  post-ready work (`boot_seed`), ahead of the ready wait. Same path for the
  SqlOnly and the Loro-authority wiring.
- No-Turso test harness (`test_environment.rs`): seeds after the scan through
  the same gate and creates only missing layout blocks.

Green after the fix: `lane-logs/sas-green-1.log` (Turso pin) and the gate logs
`lane-logs/sas-gate-*.log`.
