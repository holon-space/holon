---
id: 2026-10-12-go-home-writes-no-navigation-history-row-in-the-db-the-probes-read
date: 2026-10-12
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  In the headless frontend slice a `navigation.focus`/go-home dispatch returns
  Ok while `navigation_history` in the db handle the probes read receives no
  row at all, so 11-12 focus-dependent tests red per full-suite run.
---

## Bug

Found during lane `itests-green` (holon-space/holon-work#163) while triaging
the full `holon-integration-tests` suite on main — not by a single test
assertion but by reading the whole-suite picture: 11-12 tests of
`pbt::frontend_slice` red in every run, with the membership rotating between
runs and the family stable.

The decisive line comes from the probe
`pbt::frontend_slice::components::tests::headless_go_home_idempotency_probe`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:7862`):

    [gohome-probe] go_home×3 → 0 home(NULL) row(s) in navigation_history(main)

The dispatch itself succeeds: the probe drives
`session().execute_operation(EntityName::new("navigation"), "focus", …)`,
asserts `Ok`, settles 500 ms, then reads
`SELECT region, block_id FROM current_focus WHERE region = 'main'`
(`components.rs:7230-7300`).

Evidence logs (test profile, machine load 33-118):
`target/itests-green-baseline.txt` (35 failed, 7 timed out) and
`target/itests-green-final.txt` (33 failed, 4 timed out) in the lane
workspace.

## Root cause

NOT established. What is established:

- The BASE TABLE `navigation_history` receives no row, not merely a stale
  matview. So the defect is upstream of IVM maintenance — the usual
  "quoted-identifier write bypasses matview maintenance" shape is excluded.
- Loro routing is excluded by reading: `HeadlessFrontendComponent::new` calls
  `new_with_loro(.., false)`
  (`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:482`),
  the in-memory navigation provider is wired only into the Loro-only engine
  (`crates/holon-loro-wiring/src/loro_block_query_source.rs:172-176`), and
  the navigation tables are deliberately kept out of Loro
  (`crates/holon/src/navigation/mod.rs:33-69`). My first hypothesis — the
  typed-entity write-authority series — is therefore REFUTED by my own
  recon.

Surviving hypotheses, in probability order:

1. The write lands in a store other than the `comp.engine().db_handle()` the
   probes read — the known second-writer shape (a provider that built its own
   operations object).
2. The idempotent no-op early return in
   `crates/holon/src/navigation/provider.rs:420-426` swallows the first
   focus as "already there", so nothing is ever appended.
3. `get_current_history_id`'s `unwrap_or(0)`
   (`crates/holon/src/navigation/provider.rs:979-980`) makes an absent
   history indistinguishable from history 0, and the append targets a
   history row that does not exist.

The alternative that the PROBE reads a handle production never writes (i.e.
FALSE-ALARM) is not excluded either. Per the triage rule an unexplained
failure stays OPEN in its escape class until root-caused.

## Missing piece

No test asserts that the handle a frontend-slice probe reads is the same
handle the navigation provider writes. Each probe asserts the END state and
infers the wiring, so a store split presents as a rotating cast of focus
failures instead of one wiring red. That is a prod/test parity gap, not a
generation or oracle gap.

Second missing piece, for the gate: the family's MEMBERSHIP rotates run to
run on the same tree (the final run added `headless_go_home_idempotency_probe`,
`headless_structural_seed_and_reconcile_probe`,
`navigate_back_forward_roundtrip_lockstep_stays_green`,
`sut_only_navigate_home_is_caught_by_focus_invariants`,
`right_sidebar_renders_pins_in_declared_added_ts_order`,
`embedded_page_expand_toggle_drives_expanded` and dropped
`headless_navigate_focus_updates_matview`,
`headless_nav_history_ops_dispatch`,
`navigate_home_lockstep_stays_green`,
`journal_feed_via_main_panel_focus_shows_feed`,
`frontend_fresh_drive_org_seed_full_catalog_green`). A per-test known-reds
row over this family flaps; it needs a family-level row or the cause fixed.

## Remedy

Open. Nothing was changed: the lane's fence allowed only failures outside
org-ingest/boot/effects/supervisor/fluxdi, and this needs an owner with the
navigation-provider context. Next step for that owner: assert handle identity
(write through the provider, read through `comp.engine().db_handle()`, compare
`navigation_history` row counts) BEFORE bisecting — that one assertion
separates hypothesis 1 from 2 and 3.
