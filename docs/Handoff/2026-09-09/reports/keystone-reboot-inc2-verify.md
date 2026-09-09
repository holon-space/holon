# Verify — keystone-reboot Inc 2 (+ rev-2 fixups)

Tree under test: `b819f06e` code (identical to final `de89582fe906` except the
untouched `scripts-lane/*` files), extracted to
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/verify-reboot2`,
`target` CoW-seeded from `_sw_integ`. All runs in that tree; `pwd` printed per step.

## Verdict: CONFIRMED (4 gaps, no defects)

| Claim | Verdict |
|---|---|
| C1 fixups closed D1–D4 + C2 | CONFIRMED |
| C2 `Reboot` transition + `RefReboot` model | CONFIRMED |
| C3 gate-ON red is only `inv-no-observed-errors` | CONFIRMED (not by the command as stated — see GAP 1) |
| C4 gates | CONFIRMED |
| C5 commit hygiene | CONFIRMED |

## C1 — read each fixup in the diff, plus lane-log evidence I re-grepped

- `ComposedSlice::reboot(_: Self::Handle, _: CapMap, …)` by value (harness.rs);
  `WideE2E::reboot` and `reboot_wide(handle: WideHandle, caps: CapMap, …)` match.
- `reboot_wide` does `drop(caps); drop(handle);` then
  `assert_eq!(Arc::strong_count(&frontend), 1, …)`.
- `HeadlessFrontendComponent.boot: RwLock<Option<Arc<BootedSession>>>`; `reboot`
  `.take()`s it, `Arc::downgrade` → `drop(old)` → `assert!(dead.upgrade().is_none())`.
- Measured red FOUND: `.claude/worktrees/keystone-reboot/lane-logs/rev2-redhunt-51601.log:310`
  `[reboot-probe] frontend holders BEFORE releasing caps+handle: 34`.
- `converge_boot` factored (components.rs), used by both boots; `reboot` then
  converges on `wedge_deadline()` fail-loud and calls `settle_block_ids_stable(wedge)`
  BEFORE the anti-double-seed `after` snapshot.
- `rebooted` runs the full timed window: `sut_ids` before, `invalidate_render_caches`,
  `tokio::time::timeout(wedge, …)`, `note_settle`, `holon_latency stage=action_total`,
  `feed_sut_clock`, `sut_ids` after, then the shared `post_apply` (extracted verbatim
  from `apply`).
- `note_tick_start` hook: default no-op on the trait, `apply_transition` delegates to it,
  `rebooted` calls it LAST in its window. Pre-fix red found at
  `rev2-redhunt-43707.log:396` (`NavigateFocus.sql_ddl: 76 exceeds expected 0`).
  Post-fix line found at `rev2-redhunt-51601.log:326` AND independently reproduced by me:
  `[inv-sql-budget] Reboot: reads=0 (dedup 0)/43 writes=0/2 ddl=0/0`.
- `ObservabilityCarry` lifts all 8 caps out by `CapName` (`expect` on each) and
  `install_observability_caps(caps, Some(carry))` re-inserts the same objects and
  returns early, skipping the process-global resets.
- Dead code: my own `cargo check` shows the three Inc-1 warnings
  (`org_root`, `db_path`, `store` never used) are gone; remaining never-used warnings
  are unrelated pre-existing test helpers.

## C2 — transition and reference model

- Registered beside `SimulateRestart` in `transitions/mod.rs` (mod, `pub use`, enum
  arm, `one!(Reboot, lc::SutAppLifecycle)`).
- `simulate_restart.rs` doc comment updated — the "unbuilt F9 fork" text is replaced
  by a pointer to `transitions::Reboot`.
- Gate mirrors `HOLON_PBT_DOC_RENAME` exactly: both are
  `std::env::var("…").is_ok()` (rename_document.rs:106, reboot.rs:82).
- OFF by default proven empirically: my gate-OFF `just keystone-smoke` log contains
  zero occurrences of `Reboot`.
- jsonl replay is a comment block (`#`-prefixed) and `hand_authored.rs:196` filters
  `!line.starts_with('#')`, so `just hand-authored` does not run it — confirmed by
  the green 9-case run below.
- `RefReboot::reboot_drops_in_memory_state` clears exactly `active_editor`,
  `focused_cursor`, `seen_focus_targets`; everything else in `UITabState`
  (`navigation_history`, `next_history_id`, `focused_entity_id`, `focused_block`,
  `expanded_toggles`, `drawer_open`) is kept. Checked against boot behaviour
  EMPIRICALLY rather than by reading: in my weighted run a real `Reboot` tick executed
  and every engaged invariant passed except `inv-no-observed-errors` — had the
  kept/dropped split been wrong, `inv-focus-roots` / `inv-navigation-focus` /
  `inv-blocks-match-ref` would have red'd on that tick. See GAP 4 for the one sliver
  not covered.

## C3 — the red hunt, reproduced by me

`HOLON_PBT_REBOOT=1 HOLON_PBT_FORCE_FULL=1 HOLON_PBT_REBOOT_WEIGHT=40
PROPTEST_MAX_SHRINK_ITERS=0 HOLON_PBT_INVARIANTS="inv-drawer-open-matches-ref:warn"
just keystone-smoke` → exit 101. Log `scratchpad/vr2-red2.log:340`.
Sole failing invariant (I grepped every `("inv-…"` tuple in the whole log — count is 1):
`inv-no-observed-errors`, 15 swallowed problems, ALL `Database error: Actor channel
closed` from the dead boot's `FileSyncController` / `OrgMode` / `LoroSyncController` /
`ClockScheduler`. No other red, and `grep -c '\[reboot\]'` = 0 — neither new assert
fired. This is the production defect of
`docs/Testing/bugfunnel/entries/2026-09-08-reboot-orphans-watcher-tasks.md`, not a
harness defect.

## C4 — gates (all in the scratch tree, `pwd` asserted inside each script)

| Gate | Result | Log |
|---|---|---|
| `cargo check -p holon-integration-tests --features holon-integration-tests/pbt --all-targets` | GREEN `CHECK_EXIT=0` | `scratchpad/vr2-check.log:1067` |
| `just keystone-smoke` (gate OFF) | GREEN `ok. 4 passed; 0 failed` | `scratchpad/vr2-smoke.log:1064` |
| `scripts/keystone-known-reds.sh` | GREEN, nothing to classify | same log:1069 |
| `just hand-authored` | GREEN `ok. 9 passed; 0 failed` (1543 s), no rerun needed | `scratchpad/vr2-hand.log:6062` |
| `scripts/featuremap.py check` | GREEN `FeatureMap.md is up to date` | this session |
| `scripts/bugfunnel.py check` | GREEN `656 entries, 0 problems` | this session |

`typechars-sql-reads-budget` is registered at `KeystoneKnownReds.md:121`.

## C5 — commit hygiene

`jj -R <ws> diff -r de89582fe906 --name-only` = exactly the 12 expected files;
`grep -cE "scripts-lane|\.log$|lane-logs"` = **0**. Same file set as `b819f06e`.
FeatureMap regeneration: the commit bumps the alphabet 73 → 75 while adding only ONE
transition, and adds a `Search` bullet the commit does not otherwise touch — so it
absorbed the base's pre-existing `Search` drift, exactly as claimed.
`lane-report-keystone-reboot.md` carries `## Inc 2` (line 104) and
`## Rev 2 (Inc 1/2 fixups)` (line 327).

## Gaps (no defects found)

1. **The stated reproduction command does not reproduce.** `HOLON_PBT_REBOOT=1 just
   keystone-smoke` alone PASSES — exit 0, `ok. 4 passed`, and **zero** `Reboot` draws
   (`grep -c Reboot scratchpad/vr2-red1.log` = 0): at the default weight 1 over 4 cases
   the transition never gets drawn. The bugfunnel entry and the jsonl comment both give
   the correct weighted command, so only the claim's shorthand is wrong — but anyone
   re-verifying with the shorthand gets a false green.
2. **Error-count range understated.** The entry says 11–12 `Actor channel closed`
   ERRORs per reboot tick and the lane report says 4; I observed 15. Same class, but
   the entry's range is not the observed range.
3. **`Reboot`'s `sql_budget` bounds nothing.** It declares `reads: REACTIVE_BASE + 38,
   writes: 2, tolerance: 40 + docs_tolerance`, while the measured tick is
   `reads=0/43 writes=0/2 ddl=0/0` — by design, since the boot's cost sits outside the
   window. The lane report discloses this, but the in-code comment ("UNMEASURED at
   authoring time — widen once a run reports real numbers") is now stale in the wrong
   direction: it has been measured, and the number should shrink, not widen.
4. **`drawer_open` persistence is the one `RefReboot` claim not exercised.**
   `inv-drawer-open-matches-ref` is demoted to `warn` in the only run configuration
   that draws a `Reboot` (a disclosed softening for a registered pre-existing known red,
   `KeystoneKnownReds.md:125`). It emitted no warning on the reboot tick, so there is no
   evidence against the claim — but it is asserted, not proven.
5. Minor, mirrored not introduced: the gate is `env::var(..).is_ok()`, so
   `HOLON_PBT_REBOOT=0` also ENABLES the transition. That is exactly
   `HOLON_PBT_DOC_RENAME`'s shape, so it is a fleet convention, not this lane's bug.

## Inc 2 rev — `6a5cce80e049` (delta re-verify)

Tree extracted to `.../scratchpad/verify-reboot3`. `jj diff -r 6a5cce80e049 --stat` =
the same 12 files (`grep -cE "scripts-lane|\.log$|lane-logs"` = **0**); only 3 files
move vs the prior rev (`reboot.rs` +19, the entry +21, `keystone.jsonl` +4), so the
code seam I already exercised is untouched.

`cargo check -p holon-integration-tests --features holon-integration-tests/pbt
--all-targets` → **GREEN** `CHECK_EXIT=0` (`.../scratchpad/vr3-check.log:265`).

All four gaps CLOSED:

1. **Weighted repro stated everywhere.** `reboot.rs` type doc: "Reproducing anything
   about a reboot needs the WEIGHT, not just the gate… `HOLON_PBT_REBOOT=1` alone
   leaves the weight at 1 against ~70 other [transitions] … and passes — a green that
   says nothing." Same in the entry Remedy (line 81), the `keystone.jsonl` comment
   ("Reproduce with ALL THREE variables" + "The gate alone is NOT a reproducer…
   draws zero reboots and passes green, proving nothing"), and the report.
2. **Counts replaced by the measured range with the reason.** Entry: "measured between
   4 and 15 across runs. It varies because it counts whichever of the dead boot's
   watchers happen to be mid-read when the actor closes, plus however many of the
   org-writeback supervisor's three restart rounds fire before it gives up — not
   because the defect is intermittent." `keystone.jsonl` carries the same 4-15.
3. **`sql_budget` set from the measurement.** Now `ExpectedSql { reads: 0, writes: 0,
   ddl: 0, tolerance: 4 + docs_tolerance(state) }`; `grep -c REACTIVE_BASE reboot.rs`
   = **0** (import dropped); the stale "UNMEASURED … widen" comment is replaced by the
   measured value, its provenance log, and "n=1, so re-measure and widen with numbers,
   never pre-emptively".
4. **`drawer_open` disclosed as an open oracle gap** in the `reboot.rs` type doc
   ("the one `RefReboot` claim no run has checked … the only configuration that actually
   draws a `Reboot` softens `inv-drawer-open-matches-ref` to `warn`") and in the entry
   (lines 98-101).

The jsonl case row remains `#`-commented, so `just hand-authored` is unaffected.

**Verdict: CONFIRMED.** One residual note, not a gap: `tolerance: 4` over a `reads: 0`
expectation is tight for an n=1 measurement — if reboot post-window bookkeeping ever
issues reads, this budget reds before anyone changes the reboot. The comment already
says so and names the remedy, so this is disclosed, not hidden.
