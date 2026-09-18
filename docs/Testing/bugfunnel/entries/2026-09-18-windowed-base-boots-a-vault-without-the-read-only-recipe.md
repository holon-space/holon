---
id: 2026-09-18-windowed-base-boots-a-vault-without-the-read-only-recipe
date: 2026-09-18
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  The windowed/TUI PBT base booted a vault with no `keystone-recipe.cook` while
  its oracle seeded the recipe for every frontend draw, so the windowed SUT was
  permanently missing three ids the invariants and transitions still named.
---

## Bug

Two unregistered reds in `holon-tui::tui_ui_pbt`, both naming blocks of the
read-only recipe document — found by the `fix-drawer-open-known-red` lane and
its verifier in files their diff never touched, and reproduced + root-caused by
the `tui-novel-reds-triage` lane:

```
[read-only homes] block:keystone-recipe.cook::b::1 is declared read-only-homed
but has no block_raw row to re-write
  — crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:7095
[inv-watch-rows-match-ref] CDC UI model for watch 'query-gcxil' has wrong block
IDs. Expected 36 blocks … Got 33 … Missing in ui_model: [
  block:keystone-recipe.cook::b::1, block:5f54ed1a-…370cd,
  block:keystone-recipe.cook::b::0]
```

Reproduced deterministically by seed in this lane: the `[read-only homes]`
panic at `PROPTEST_SEED` 4, 12, 28, 32 and the watch divergence at 10 (4/36 and
1/36 of an unbiased 36-run sweep; logs `lane-logs/tui-sweep-{4,10,12,28,32}.log`
of `.claude/worktrees/tui-novel-reds-triage`).

## Root cause

Seed parity between the two wide bases was broken for the read-only document.

- `wide_e2e_ref_for` seeds the recipe into the ORACLE for **every frontend
  (ViewModel) draw**: `wide_e2e.rs:1764` calls `seed_read_only_recipe`, which
  inserts the recipe page (`block:5f54ed1a-…`, a path-derived id) plus its two
  steps and marks both steps read-only-homed.
- `boot_and_seed_wide` (headless) keys the on-disk file on the same condition —
  `wide_e2e.rs:1145-1147` pushes `(READ_ONLY_RECIPE_FILE, KEYSTONE_RECIPE_COOK)`
  when `!ref_state.read_only.homes().is_empty()`.
- `boot_and_seed_wide_windowed_base` (the TUI + gpui windowed base) pushed only
  `structural-page.org` and `forward-edge-page.org` (`wide_e2e.rs:1448-1456`, and
  identically at the lane's base `main-` = `4b01bbdc1b31`). No `.cook` file ever
  reached the windowed vault.

With the file absent, the recipe's page and steps never ingest, so they are
absent from `block_raw`. The oracle still models all three, and the
`SutReadOnlyEditAttempt` cap is inserted for every composed base
(`composed/builder.rs:602`), so `AttemptIngestCompoundOnReadOnly` remains
generatable and its `apply_to_sut` reads `SELECT content FROM block_raw` for a
step that has no row → the panic. `inv-watch-rows-match-ref` sees the same three
ids missing from the watch's `ui_model`. Prod is correct throughout: it ingests
whatever the vault holds, and the vault held no recipe.

Why it stayed hidden: the headless keystone has the file, so only the windowed
tier can draw this; and the windowed start barrier
(`wide_e2e.rs:1511-1521`) expected only the forward-edge corpus + the boot
journal, so a dropped recipe ingest was neither awaited nor reported.

## Missing piece

A single source for "the files this wide vault boots with". Two boots built
their seed lists independently, and only the windowed one lost the read-only
arm — with no barrier covering the ids whose absence only surfaced later as an
unrelated-looking panic.

## Remedy

FIXED (`tui-novel-reds-triage`). `boot_and_seed_wide_windowed_base` now pushes
the recipe file under the same oracle-keyed condition as the headless boot, and
its `start_expected` barrier includes `ref_state.read_only.homes()`, so a
genuinely-dropped recipe ingest fails loud at boot instead of mid-scenario.

Evidence: pre-fix seeds 4, 12, 28, 32 fail on the read-only panic and seed 10 on
the watch divergence; post-fix all five are clean, four of them under a machine
load of ~50 (`lane-logs/post-sweep-*.log`). Post-fix, seed 7 draws
`AttemptIngestCompoundOnReadOnly` at step 1 and passes. A `ToggleState:400`-
biased post-fix sweep passed 8/8 including one draw of the same transition.

One consequence of the recipe now really being in the windowed vault: a
`SetEdgeField` draw can aim `set_field` at a step and the read-only gate refuses
it (`op_write_cap.rs:513`, `cooklang is a read-only format`), 2/48 biased runs at
load 79-111. The classifier absorbs that as the pre-registered
`cooklang-read-only-write-refusal-any-op` (PASS-WITH-NOTE), whose own row text
already names exactly this as the unregistered part ("the refusal itself is
CORRECT behaviour … what is unregistered is that a draw seats the write there at
all") — a known red newly reachable in the windowed tier, not a new red.

Residual, NOT fixed here: the same two-boot divergence still omits
`Journals.org`, the folder-companion date file and `ref_state`'s extra org files
from the windowed seed list. No red is attributed to them today (the windowed
sweep runs green on `inv-blocks-match-ref/org`), so they are flagged, not
changed.
