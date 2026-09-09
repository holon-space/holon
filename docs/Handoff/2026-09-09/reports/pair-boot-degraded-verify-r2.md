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

# Rev 3

Same workspace, base `c6daff82` (= `sw/quick-open-focus`, wave-10 tip; contains
rev 1+2). `jj workspace update-stale` → "not stale". Rev 3 uncommitted on `@`,
6 files. No jj/git write command; every probe was cp + sha256, restore verified.
Logs: `…/boot-degraded-verify-r2-logs/r3-*`.

## Claim 1 — the root sentinel is a home — CONFIRMED

Teeth (`r3-s1b.log`, my own revert of ONLY the predicate back to
`if placed.contains(id) || !placed.contains(snap.block.parent_id.as_str())`):
- baseline 18/18 pass (`:87-88`, `BASE_EXIT=0`);
- mutant RED at BOTH tiers (`MUTANT_EXIT=100`, `:212-216`):
  unit `device_pairing_op.rs:1413` `left: []  right: ["block:page", "block:note"]`
  (`:201-204`); integration `pairing_boot_degraded.rs:322` `block:phone-page hangs
  under the root sentinel … the store holds: ["block:owner-root"]` (`:177-178`);
- sha256 `4cca7b22…9347f` before == after, `RESTORE VERIFIED`, restored run 18/18
  (`:217-218`, `RESTORED_EXIT=0`).

My own probes (scratch `crates/holon-loro/tests/zzv3_scratch.rs`, run then DELETED
— `ls crates/holon-loro/tests/` is back to the three tracked files); `r3-s2.log`,
`SCRATCH3_EXIT=0`, 2/2 pass:
- **id collision on a sentinel-parented page** (`:115-120`): archive holds
  `block:shared-page` ("PHONE version") + a child; the adopted store holds
  `block:shared-page` ("OWNER version"). Result: `block:shared-page` count 1 (no
  duplicate top-level page), `block:shared-page-before-pairing` count 1, its
  `parent_id = EntityUri("block:shared-page")` (not the sentinel), properties
  `{"pairing_conflict_of": String("block:shared-page")}`. Conflict-copy rules win
  over the new sentinel rule.
- **arithmetic on my own fixture** (`:105-106`): archive = 3 extra top-level pages
  + 1 child + the journals subtree. All 4 placeable blocks reach the store;
  `orphans` = exactly `["block:phone-note (parent block:2026-09-05)",
  "block:2026-09-05 (parent block:journals)"]` = 2, and the raised condition
  carries 2. The lane's "2 not 4" arithmetic is right.

## Claim 2 — full-width bar, no overlap — CONFIRMED (caret probe INCONCLUSIVE)

`r3-s3.log`, `frontends/gpui/src/share_ui.rs`, sha256 `43dc4c48…d1e94` before ==
after, `RESTORE VERIFIED` (`:629-630`):
- baseline `test result: ok. 1 passed … 6.15s` (`:206`, `BASE_WINDOWED_EXIT=0`);
- mutant A (`.absolute().top(px(16.0)).left(16).right(16)`): RED at
  `pairing_deferred_reimport_windowed.rs:333` — `the bar must not cover the title
  row; the bar spans y 16..76 and the title row spans y 0..38`
  (`:405-407`, `MUTANT_TOP16_EXIT=101`);
- mutant B (same, `top(px(40.0))`): RED at `…:352` — `the bar must not cover
  content; it spans y 40..100 and text-block:journals-content spans y 46..72`
  (`:615-617`, `MUTANT_TOP40_EXIT=101`).
Both reds reproduce the lane's claims verbatim, at two DIFFERENT assertions.

Caret/focus: **not established either way by my instrument.** Scratch windowed
tests (both written, run, DELETED). With the bar painted, dispatching printable
keystrokes after a click on `text-block:journals-content` produced no text change
(`r3-s4b.log:215-219`, assertion failed at my own scratch line 405). CONTROL with
NO banner at all: identical — `V3-CONTROL landed = false` (`r3-s5.log:292-294`,
test itself `ok`). Typing does not reach the editor in this harness regardless of
the bar, so the probe cannot separate the two; it is NOT evidence of a defect.
Static evidence for the claim: `render_deferred_reimport_bar`
(`frontends/gpui/src/share_ui.rs:1245-1360`) contains no `track_focus`,
`focus_handle`, `key_context`, `on_key_*`, `on_action`, `occlude` or
`block_mouse` — it is a non-absolute `div().id().w_full()` in the page flex
column, so there is no mechanism by which it takes keyboard focus.

## Claim 3 — the three moved fixtures — BY DESIGN, with a bounded residual risk

Both substitute parents are permanently excluded from the re-import by
`own_content` (`crates/holon-loro/src/device_pairing_op.rs:591-620`):
- `block:journals` is `JOURNALS_MACHINERY[0]` (`device_pairing_op.rs:112,118-124`)
  — the `holon-loro` fixture (`tests/pairing_boot_degraded.rs`);
- `block:root-layout` is a bundled layout id: `bundled_layout_ids()`
  (`device_pairing_op.rs:96-108`) parses `include_str!("../../../assets/default/index.org")`,
  whose line 3 is `:ID: root-layout`. Used by the windowed pin and the boot_suite
  member.
So this is NOT a fixture hack that breaks "when journals get re-imported": these
ids are never carried by the re-import, by construction.

Residual risk, reported not remedied: unplaceability ALSO requires the ADOPTED
store not to hold the id. The windowed pin asserts that precondition explicitly
(`pairing_deferred_reimport_windowed.rs`, "precondition: the archived page must
have nowhere to go"). The boot_suite member does NOT assert it, but its bus
assertion demands `orphans == OWED_BLOCKS`, so a store that gained the layout root
would fail it loudly rather than silently complete.

## Claim 4 — gates — CONFIRMED (`r3-s6.log`)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | `FMT_EXIT=0` (`:2`) |
| `-p holon-loro -p holon-loro-wiring -p holon-sharing -p holon-app` | `Summary [56.318s] 624 tests run: 624 passed (1 slow), 4 skipped`, `GATE4_EXIT=0` (`:732-733`) |
| `-p holon-integration-tests --test boot_suite` | `Summary [49.790s] 22 tests run: 22 passed (1 slow), 0 skipped`, `BOOTSUITE_EXIT=0` (`:927-928`) |
| `just keystone-smoke` | `test result: ok. 4 passed … 1.91s`, `KEYSTONE_EXIT=0` (`:1149-1151`) |
| `just loro-suite` | `Summary [4.077s] 13 tests run: 13 passed, 1 skipped`, `LORO_SUITE_EXIT=0` (`:1336-1337`) |

Failure set is EMPTY. None of the known signatures appeared, including
`test_turso_backend_state_machine` and
`a_dispatched_switch_reaches_the_seeded_section`. Rev 2's `boot_suite` red
`mirrored_entity_keeps_the_connector_as_its_only_write_authority` also passes on
this base — it was fixed upstream in wave 10, not by this lane.

## DEFECTS / GAPS (Rev 3)

1. GAP — the caret/focus half of dogfood entry
   `2026-09-08-degraded-banner-covers-the-titlebar-and-first-row` has NO test and
   I could not build one: keystrokes do not reach the editor in this windowed
   harness even with no banner (`r3-s5.log:294`). Only geometry is pinned.
2. GAP — after rev 3 the deferral path is close to unreachable in a REAL store:
   any archived subtree whose top ancestor is sentinel-parented is now fully
   placeable, and a production store holds `block:journals` and
   `block:root-layout`. All three fixtures therefore rely on an adopted store that
   LACKS machinery a real store has. The banner/retry code is pinned only against
   that artificial shape (and the lane still has no dogfood pass for it).
3. GAP (pre-existing, unchanged by rev 3) — a conflict copy separates a family:
   with a diverged top-level page, the phone's page content goes to
   `block:shared-page-before-pairing` while its child `block:phone-child` is
   re-parented under the OWNER's `block:shared-page` (`r3-s2.log:118-120`). That
   is the documented uniform re-parent rule, but no test states it for the new
   sentinel case.
4. NOTE — `boot_suite` is still not in the lane's gate command although this lane
   owns a member of it.
