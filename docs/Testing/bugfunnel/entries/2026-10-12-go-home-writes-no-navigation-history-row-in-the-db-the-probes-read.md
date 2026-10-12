---
id: 2026-10-12-go-home-writes-no-navigation-history-row-in-the-db-the-probes-read
date: 2026-10-12
gap: ENVIRONMENT
secondary: null
status: FIXED
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

ESTABLISHED (lane `nav-focus`): the probes drive navigation BEFORE the boot
seed has written the region's first `navigation_history` row, and `go_home`
against an empty history is a silent no-op.

The headless frontend boots `without_wait()`, so `seed_default_layout` — whose
last act is `navigation::focus(Main, block:journals)`, the FIRST history row
(`crates/holon-app/src/seed.rs:176-204`) — runs on a spawned `post_ready_work`
task (`crates/holon-app/src/wiring.rs:813-817`). The harness only paid a flat
`settle` budget that tolerates non-convergence, so under load the seed lands
after the driven interactions.

With the history still empty, `NavigationProvider::focus`'s idempotency guard
(`crates/holon/src/navigation/provider.rs:398-426`) reads the cursor's current
row, gets NO row, and compares `None == block_id`: for `go_home`
(`block_id = None`) that is "already home", so it returns before any write —
permanently, since nothing else seeds the region.

The decisive evidence is the red log's own row dump: ONE row, `id=1
block_id=block:journals`, and zero home rows. The seed row is present at
read time but was written AFTER all three `go_home` calls — had it been there
first, `go_home×1` would have inserted a home row. That also explains the
rotating family membership: it is a load-dependent race, not a fixed set.

Hypotheses 1 and 3 below are both REFUTED. The write targets the same handle
the probes read (the seed's own row arrives in it). `get_current_history_id`'s
`unwrap_or(0)` is CORRECT and load-bearing: AUTOINCREMENT ids start at 1, so 0
is a before-the-first-row sentinel that lets `go_forward` out of home (which
`nullify_cursor` leaves) find row 1 via `id > $current_id`. Making it fail
loud, as first proposed, would break forward navigation out of home.

What was established by the original triage:

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

The headless harness had no ORDERING over the boot seed — only a time budget
that explicitly tolerates non-convergence. Every frontend-slice probe asserted
its end state and inferred that the boot it depends on had happened, so a
boot step that had not landed yet presented as a rotating cast of focus
failures instead of one ordering red. That is a prod/test parity gap, not a
generation or oracle gap.

(The original triage guessed a split db handle here; that was wrong — see
Root cause. The handle was never the issue, the ORDER was.)

Second missing piece, since resolved: the family's MEMBERSHIP rotated run to
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
row over this family would flap — the rotation WAS the race's signature, so
the ordering fix removes the need for a family-level row.

## Remedy

`HeadlessFrontendComponent::boot_session` now BLOCKS until the boot seed has
ended, through the engine's existing `LayoutSeed` phase watch
(`await_layout_seed`, `crates/holon-integration-tests/src/pbt/frontend_slice/components.rs`),
before the boot settle. The phase is `Pending` from inside the session factory,
so the wait cannot observe the pre-`begin` `Settled`; a failed or skipped seed
settles too (the guard settles on drop), so nothing can park. A 120 s timeout
fails loud.

Production is UNCHANGED. The only prod-side edit is a comment on
`get_current_history_id` naming 0 as the before-the-first-row sentinel, so the
next triage does not mistake it for a silent default again.

Red → green on `headless_go_home_idempotency_probe`: 0 home rows before,
1 after (rows `id=1 block:journals`, `id=2 NULL`), test profile.

Still true, and deliberately left: `go_home` on a genuinely empty history
writes nothing. The display is already home and the reference model agrees
(`nav_go_home` skips when `current_focus` is `None`), so this is the contract,
not a defect — it only became visible because the harness let the empty state
be observed.
