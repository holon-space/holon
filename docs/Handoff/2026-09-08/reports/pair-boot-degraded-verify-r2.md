# Adversarial re-verification — pair-boot-degraded Rev 2 (D94.a)

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-boot-degraded`
(identity: `crates/holon-integration-tests/tests/boot_suite/pairing_deferred_reimport_boots.rs`
present; `DEGRADED_TOAST_STACK` in `frontends/gpui/src/share_ui.rs`).
Logs: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/boot-degraded-verify-r2-logs/`
No jj/git write command was run. All probe mutations were cp + sha256, restore verified.

## Claim 1 — mixed archive, totality, crash safety — CONFIRMED

Own scratch test (`crates/holon-loro/tests/zzv2_scratch_mixed.rs`, written, run,
then DELETED — `s2.log` for the run, `ls crates/holon-loro/tests/` now shows only
`layout_create_idempotence.rs`, `layout_migration.rs`, `pairing_boot_degraded.rs`).

Archive built by me: 2 placeable (`block:phone-todo` under the shared
`block:owner-root`, `block:phone-todo-2` under it — so the fixed-point loop is
actually needed) + 3 unplaceable (`block:2026-09-05`, `block:phone-note`,
`block:phone-note-child`).

`s2.log:137-141`
- outcome `Deferred { orphans: [phone-note, 2026-09-05, phone-note-child] }` — 3.
- store after boot = `{block:phone-todo: 1, block:phone-todo-2: 1, block:owner-root: 1}`
  → both placeable blocks are IN, each exactly once.
- bus = `PairingReimportDeferred { orphans: 3, archive: <exact marker.archive> }`,
  asserted field-wise; count is exactly 3.
- marker present = true; archive snapshot present = true.

NOTE on counting semantics (probed, not a defect): `block:journals` is in
`JOURNALS_MACHINERY` and is excluded from `own_content`
(`device_pairing_op.rs:591-620`), so it is deliberately neither re-created nor
counted. A first run with journals in the archive showed `orphans: 2`
(`s1.log:151-153`) until I deepened the subtree — the count is over USER content,
which is the right set for a banner.

Crash safety / idempotence (`s2.log`):
- `v2_idempotent_double_completion`: two `complete_interrupted_pairing` calls on
  the same store — `after_a == after_b`, no id with count > 1 (`s2.log:125-128`).
- `v2_divergent_conflict_copy_not_duplicated_across_deferred_boots`: archive holds
  a DIVERGENT `block:owner-root`; three deferred completions in a row produce
  `{owner-root, owner-root-before-pairing, phone-todo, phone-todo-2}` unchanged
  across all three (`s2.log:111-116`). No conflict copy is written twice.
- Marker-truncation kill-point: after a deferred boot I truncated
  `pairing-in-progress.json` to half its bytes. `read_marker` → `Err(… is not a
  pairing marker)`, `pair_retry_reimport` → same `Err`, NOT "owes no pairing
  re-import" (`s2.log:150-155`). Loud, not a silent "nothing owed".
- D78.d kill-point matrix `pairing_swap::tests`: 5/5 PASS (`s3.log:68-75`,
  `SWAP_EXIT=0`), including `a_kill_before_the_archive_rename…`,
  `a_kill_between_the_two_renames…`, `a_kill_after_the_promote_still_owes_the_reimport`.

No path found where a placeable block is written twice or lost.

## Claim 2 — windowed pin — CONFIRMED, and the lane's stated remaining gap is CLOSED

- `grep -c "pair_retry_reimport(" frontends/gpui/tests/pairing_deferred_reimport_windowed.rs`
  = **0**. The only direct production call in the test is one
  `complete_interrupted_pairing` at line 263 (the seeded boot), and the store/bus/
  `DevicePairing` all come from the composed session (lines 224/227/230).
- Baseline green: `s5.log:184` `test result: ok. 1 passed … 6.19s`.
- My own neuter of `dispatch_retry_reimport` (`if true { return; }` at the top of
  the body): RED at the CLICK assertion —
  `s5.log:370` `panicked at frontends/gpui/tests/pairing_deferred_reimport_windowed.rs:313:9:
  the press must tell the user which blocks are waiting on what; the toast stack
  painted "" and does not name block:pair-owed-page`, `MUTANT_DISPATCH_EXIT=101`,
  sha256 `adeba84b…9aac39` before == after, `RESTORE VERIFIED` (`s5.log:382-383`).
- Click #2 isolated (the lane called this unproven): I instead neutered
  `self.bus.clear(&Self::deferred_condition())` in `device_pairing_op.rs`. RED at
  `s6.log:195` `pairing_deferred_reimport_windowed.rs:361:5` — the "banner lifts"
  assertion — which means the preceding click-#2 assertions (marker gone, archived
  note in the store) had already passed THROUGH the button. `MUTANT_CLEAR_EXIT=101`,
  sha256 `adc170bc…657a6` before == after, `RESTORE VERIFIED` (`s6.log:206-208`).
  The lane's "click #2 not separately mutation-proven" gap does not stand.
- Accessors: `degraded_bus()` (components.rs:1075), `device_pairing()` (:1084),
  `loro_doc_store()` (:1050) are plain `pub`, NOT `#[cfg(test)]`/feature-gated.
  They do NOT widen a production API: `holon-integration-tests` is a workspace
  test-support crate — a `[dev-dependencies]` entry in `frontends/tui/Cargo.toml:60`
  and an OPTIONAL dep behind the `pbt` feature in `frontends/gpui/Cargo.toml:98,129`.
  Nothing links it into a shipped build.

## Claim 3 — boot arm — CONFIRMED

Baseline: `s3.log:219-222` `PASS … pairing_deferred_reimport_boots::a_pair_whose_reimport_has_no_home_boots_and_raises_the_banner`, `BOOT_EXIT=0`.
My own mutation of the `PairingCompletion::Deferred` arm in
`crates/holon-loro-wiring/src/loro_module.rs` (`if true { panic!("D94-MUTANT: the
deferred arm is gone"); }`): RED at `s4.log:166`
`panicked at crates/holon-loro-wiring/src/loro_module.rs:330:39`,
`Summary 1 test run: 0 passed, 1 failed, 21 skipped`, `MUTANT_BOOT_EXIT=100`.
sha256 `1b3e7391…bc8e8` before == after, `RESTORE VERIFIED` (`s4.log:176-177`).
The test therefore drives the real `LoroModule` Deferred arm, not the library call.

## Claim 4 — the "pre-existing red" — CONFIRMED pre-existing

`mcp_mirrored_entity_write_authority::mirrored_entity_keeps_the_connector_as_its_only_write_authority`
- Lane tree ×3: FAIL 3/3, all at `crates/holon/src/api/operation_dispatcher.rs:1655:21`
  (`s6.log:388,556,724` `LANE_MCP_RUN{1,2,3}_EXIT=100`); message
  (`s6.log:380`): `[OperationModule] write authority for free-standing type
  'fk_fake_shadow': … declares no `properties` overflow column …`.
- Chain-tip `_sw_integ` ×3 (read-only, cargo only): FAIL 3/3, same site
  `operation_dispatcher.rs:1655:21` (`s7.log:175,343,511`
  `INTEG_MCP_RUN{1,2,3}_EXIT=100`); message (`s7.log:167`) identical in shape,
  naming `'fk_fake_readonly'` instead of `'fk_fake_shadow'`.
Same failure site and same class in both trees, deterministic 3/3 on each → the
lane is not the cause.

`just loro-suite` was green in my run, so no A/B was needed for it. Full
`boot_suite` here: `Summary [20.858s] 22 tests run: 21 passed, 1 failed`
(`s9.log:186`) — the only red is the mcp one above;
`junction_survives_reboot_repro::loro_written_edge_fields_survive_reboot_over_existing_db`
PASSED, so the lane's contention-flake claim for it holds in my run.

## Claim 5 — gates — CONFIRMED

- `cargo fmt --all -- --check`: `FMT_EXIT=0` (`s3.log:2`).
- `cargo nextest run --no-fail-fast -p holon-loro -p holon-loro-wiring
  -p holon-sharing -p holon-app`: `Summary [64.024s] 621 tests run: 621 passed
  (1 slow), 4 skipped`, `GATE4_EXIT=0` (`s8.log:709-710`). Failure set is EMPTY,
  hence trivially a subset of the allowlist. None of the eight allowlisted names
  (incl. `quick_open_search_at_vault_scale`) failed this round.
- `just loro-suite`: `Summary [5.241s] 13 tests run: 13 passed, 1 skipped`,
  `LORO_SUITE_EXIT=0` (`s8.log:869-870`).
- `just keystone-smoke`: `test result: ok. 4 passed; 0 failed … 6.39s`,
  `KEYSTONE_EXIT=0` (`s8.log:1039-1041`).

## DEFECTS / GAPS

1. GAP — the pre-existing mcp red is not deterministic in WHICH fake type it names
   (`fk_fake_shadow` in this tree, `fk_fake_readonly` in `_sw_integ`), so a future
   A/B by message string alone can mis-attribute it. Site + class are stable.
2. GAP — `boot_suite` is not part of the lane's gate command even though this lane
   adds a member to it; the new boot test only runs if someone runs boot_suite by hand.
3. GAP — on the Deferred path no `PairingRecord` is written (`device_pairing_op.rs:966-976`
   is reached only on Completed). A device that stays deferred indefinitely is never
   recorded as its owner's; nothing in the diff pins what that means for anything that
   reads the record.
4. GAP — `degraded_bus()` / `device_pairing()` / `loro_doc_store()` are ungated `pub`
   in `crates/holon-integration-tests/src/pbt/frontend_slice/components.rs` and each
   returns `Option` via `.ok()` on the DI resolve. Harmless here (test-only crate,
   matching the file's existing `ALLOW(ok)` convention) but it does swallow the
   resolve error rather than reporting it.
5. GAP — no dogfood-explorer pass (lane admits this); the retry action is still only
   on the banner, not in Settings.
