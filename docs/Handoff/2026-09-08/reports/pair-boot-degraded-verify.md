# Adversarial verification — pair-boot-degraded (D94.a)

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-boot-degraded`
(identity sentinel `PairingReimportDeferred` present; `jj diff -r @ --stat` =
the 7 lane files, unchanged before and after this pass).
Logs: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/boot-degraded-verify-logs/`

## Claim 1 — only `ReimportHasNoParent` degrades — CONFIRMED

Scratch test `crates/holon-loro/tests/zz_verify_scratch_d94.rs` (written, run,
then DELETED — see the final `jj diff -r @ --stat` above and
`ls crates/holon-loro/tests/`: only `layout_create_idempotence.rs`,
`layout_migration.rs`, `pairing_boot_degraded.rs` remain; a copy is kept
outside the tree at `boot-degraded-verify-logs/zz_verify_scratch_d94.rs.kept`).

`scratch-v4.log`:
- `probe_corrupt_archive_still_stops_boot` PASS —
  `complete_interrupted_pairing` returned `Err`:
  `opening the pre-pair document …/holon_tree.loro to re-import from: Decode error: (Invalid magic bytes)`;
  bus empty (no deferred banner raised).
- `probe_missing_archive_still_stops_boot` PASS — `Err … No such file or directory (os error 2)`.

Mechanism read (`crates/holon-loro/src/device_pairing_op.rs:947-956`):
`e.downcast::<PairingRefused>()` only matches a bare `PairingRefused`; the
archive-open leg wraps in `.with_context`, so it can never be mistaken for the
deferred outcome. Other `PairingRefused` variants take `Ok(other) => Err`.

## Claim 2 — deferred boot keeps everything, store usable — CONFIRMED, with a DEFECT

`probe_deferred_boot_n3_keeps_marker_archive_and_store_usable` PASS with N=3
(`scratch-v4.log`): orphans =
`["block:note-a (parent block:2026-09-05)", "block:2026-09-05 (parent block:journals)", "block:note-b (parent block:note-a)"]`;
`ShareDegradedReason::PairingReimportDeferred{orphans:3, archive:<exact marker.archive>}`
asserted field-wise (not by substring); `pairing-in-progress.json` present;
`holon_tree.loro` in the archive present; no pairing record written; a write of
`block:after-boot` plus a read back succeeded.

Boot-time raise reaches the window: the GPUI bridge replays
`bus.subscribe().current` before pumping live changes
(`frontends/gpui/src/share_ui.rs:599-610`), so a condition raised during boot DI
is not lost.

**DEFECT — the banner undercounts and withholds placeable content.**
`probe_mixed_archive_count_vs_reality` (`scratch-v4.log`), archive =
`owner-root → placeable` (parent IS in the adopted store) plus an unplaceable
`journals → 2026-09-05 → note-a`:

```
MIXED: reported orphans = 2 (["block:note-a (parent block:2026-09-05)", "block:2026-09-05 (parent block:journals)"]);
       placeable block in store = false; store ids = ["block:owner-root"]
```

`plan_reimport` (device_pairing_op.rs:346-353) returns `Err` before ANY write,
so the one block that could have been re-imported right now is not, and stays
out until the unrelated `block:journals` appears. The banner text
(`share_ui.rs:1343-1348`) is
`"{orphans} block(s) from this device are not in the paired store"` — here it
says 2 while 3 archived blocks are absent. The count is the orphan count, not
the not-in-store count.

## Claim 3 — retry — CONFIRMED

`probe_two_deferred_retries_in_a_row` PASS (`scratch-v4.log`):
- retry #0 and #1 both `Err`, both naming every orphan and the wanted parent:
  `the re-import cannot place 3 block(s) …: block:note-a (parent block:2026-09-05), block:2026-09-05 (parent block:journals), block:note-b (parent block:note-a)`;
  marker still present, sticky condition still standing after each.
- After `block:journals` is written: retry succeeds, marker gone,
  `block:2026-09-05`/`block:note-a`/`block:note-b` all in the store, deferred
  condition cleared.
- Retry again → `Err`: `this device owes no pairing re-import: there is no
  \`pairing-in-progress.json\` in its store …`.
`probe_retry_with_no_marker_is_loud_not_a_panic` PASS (no marker at all → same
loud refusal, no panic, store untouched).

## Claim 4 — sticky, undismissable banner in GPUI — PARTLY REFUTED

- Banner text: CONFIRMED. `frontends/gpui/tests/pairing_deferred_reimport_windowed.rs`
  passes standalone (`gpui-v1.log`, `test result: ok. 1 passed … 14.83s`).
  The lane's teeth script IS idempotent and sha256-restoring; I ran it twice.
  Run 2 (`teeth-v2.log`) reproduces the claimed red exactly:
  `panicked at …/pairing_deferred_reimport_windowed.rs:355:5`, `MUTANT_EXIT=101`,
  sha256 before == after (`e3ff750e…fedcc`), `RESTORE VERIFIED`.
  Run 1 (`teeth-v1.log`) went red at the harness instead
  (`pbt_harness/windowed_wide.rs:79: window never reached a fixed point within
  30s: 68 elements`, preceded by `[GPUI] pre-warm timeout`) — a contention flake
  in the windowed harness, not the mutation. Noted as a flakiness hazard for
  this pin.

- **REFUTED: the Retry button's behaviour is not pinned.** My own mutation
  (`mutate-dispatch.sh` / `mutate-dispatch-v1.log`): I neutered
  `dispatch_retry_reimport` to an immediate `return` (cp aside + sha256,
  no `jj restore`). The windowed test STILL PASSES:
  `test result: ok. 1 passed; 0 failed … 16.65s`, `MUTANT_DISPATCH_EXIT=0`,
  sha256 before == after `e3ff750e…fedcc`, `RESTORE VERIFIED`.
  Cause: the test clicks the button, but every state change it then asserts is
  produced by the test's OWN direct calls —
  `pairing.pair_retry_reimport()` at lines 383 and 412 of the test. The
  clicked button dispatches through `bundle.session`'s DI-registered
  `DevicePairing` (the composed harness's own store, which has no marker), not
  the test-built `pairing` over `store_dir`. Both click assertions are
  therefore vacuous: "the banner survives the press" holds because the press
  does nothing, and "the banner lifts" is caused by the direct call.

## Claim 5 — `render_overlays` signature — CONFIRMED

`grep -rn render_overlays frontends crates --include "*.rs"`: exactly two hits —
the definition (`share_ui.rs:1137`) and one caller (`lib.rs:1616`), which passes
`self.bounds_registry.clone()`. `cargo check -p holon-gpui --all-targets`
`CHECK_EXIT=0` (`gpui-v1.log`). `degraded_bus_bridge_windowed`
`test result: ok. 1 passed … 11.15s`, `BRIDGE_EXIT=0`.

## Claim 6 — fmt + gate + loro-suite — MOSTLY CONFIRMED, one deviation

- `cargo fmt --check` on the lane tree: `FMT2_EXIT=0` (`gate-v1.log`). CONFIRMED.
  (`gpui-v1.log`'s `FMT_EXIT=1` was my scratch file only, now deleted.)
- `cargo nextest run --no-fail-fast -p holon-loro -p holon-loro-wiring
  -p holon-sharing -p holon-app` (`gate-v1.log`):
  `Summary [173.364s] 619 tests run: 618 passed (2 slow), 1 timed out, 4 skipped`,
  `GATE_EXIT=100`. **The failure set is NOT a subset of the allowlist**: the one
  red is `holon-app::quick_open_search_at_vault_scale` (`TIMEOUT [120.033s]`),
  which is not in the given set. No iroh failure occurred in my run, so no
  3× isolation replay was needed for those.
  Isolated replay (`qo-v1.log`): `PASS [37.943s] … 1 test run: 1 passed (1 slow)`,
  `QO_EXIT=0` — a vault-scale perf test that only times out under build
  contention, not a lane regression. Reported as a deviation from the stated
  allowlist, not as a defect of this change.
- `just loro-suite` (`qo-v1.log`): `Summary [6.062s] 13 tests run: 13 passed,
  1 skipped`, `LORO_SUITE_EXIT=0`. CONFIRMED.

## Claim 7 — windowed test fidelity — GAP CONFIRMED (and larger than stated)

The windowed test boots `compose_sut_windowed_base_seeded`
(`pairing_deferred_reimport_windowed.rs:291`) for the window, then builds its
own `DevicePairing::new` (line 200) over a separate `tempfile::tempdir()` and
calls `pairing.complete_interrupted_pairing(&marker)` directly (line 333).
`grep -rl complete_interrupted_pairing frontends crates --include "*.rs"` returns
only `pairing_deferred_reimport_windowed.rs`, `loro_module.rs`,
`device_pairing_op.rs`, `pairing_boot_degraded.rs`.

GAPS:
1. The production boot site — `LoroModule::configure`'s marker read, the
   `.expect(…)` and the new `match done { Completed | Deferred }` arm in
   `crates/holon-loro-wiring/src/loro_module.rs` — is exercised by NO test in
   the diff. Nothing pins that a `Deferred` outcome at real boot does not stop
   the app; only the library call is pinned.
2. Beyond the lane's stated stitch, the retry BUTTON path
   (`dispatch_retry_reimport` → `FrontendSession::execute_operation` →
   the DI-registered `device.pair_retry_reimport`) is likewise untested — proven
   by the neuter mutation above.
