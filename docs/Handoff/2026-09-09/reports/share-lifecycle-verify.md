# Verify — share-lifecycle lane (adversarial, fresh context)

pwd for every verdict: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/share-lifecycle`

## Preconditions
- `@-` = `830d794f878f` OK. `crates/holon-loro/src/share_credentials.rs` present OK.
- Patch integrity **OK**: regenerated `jj diff --git -r 5644b6d3` in the `sharing-admit`
  workspace; content byte-identical to `lane-logs/sharing-admit-rev2.patch`
  (sha `c6527284589e…`). My first hash differed only because `2>&1` captured 4 lines of
  jj snapshot stderr. `5644b6d3debf` = `sw/sharing-admit | fix(sharing): gate every iroh
  peer import on an admit decision`. Lane IS rebased on the woven commit exactly.
- Tree assert **literally FAILS**: `rg 'ShareAdmission::Ungated' crates --glob '!**/tests/**'
  --glob '!**/*test*'` returns 9 hits (iroh_advertiser.rs 81,153,165,223,254,450,624;
  peer_import.rs 25,87). The globs do not exclude in-file `#[cfg(test)] mod tests`
  (starts iroh_advertiser.rs:486). Substantively no PRODUCTION call site is ungated.

## C1 — every subtree-share ingress gated: **CONFIRMED (production call sites)**
Enumerated: `loro_share_backend.rs` 1899 / 1995 / 2623 (`share_subtree`,
`accept_shared_subtree`, `rehydrate_shared_trees`) all `ShareAdmission::Enrolled`;
1365 + 2025 `sync_doc_initiate_enrolled`; `container_registry.rs:245` `start_share_gated`;
`iroh_advertiser.rs:374` accept_loop gates at 414–453 (Enrolled → `acceptor_enroll`, else close).
`start_share_ungated` (154) has only test callers (527, 554, 624).

## C1b — visibility: **REFUTED — the earlier verifier's flag is UNFIXED**
Reachable from any other crate, bypassing the gate:
- `AdmittedPeer::ungated` — `pub` (peer_import.rs:88) on `pub struct AdmittedPeer` (:45),
  `lib.rs:96 pub mod peer_import;` → `holon_loro::peer_import::AdmittedPeer::ungated(..)`.
- `sync_doc_accept` — `pub` (iroh_sync_adapter.rs:370), re-exported `:1115`,
  `lib.rs:65 pub mod iroh_sync_adapter;`. Same for `sync_doc_initiate` (:1119) and
  `start_share_ungated` (lib.rs:60).
These should be `pub(crate)` or `#[cfg(any(test, feature="…"))]`.

## C2 — secrets: **CONFIRMED**, except its test claim: **REFUTED**
- Keychain real: `share_credentials.rs:51-56` → `holon_secrets::platform_keychain`
  ("space.holon.share-capability"); macOS Security.framework, else `keyring`. No file/prefs
  fallback; `in_memory()` is `#[cfg(any(test, feature="test-helpers"))]`; unsupported
  platform returns `Err`, not `None`.
- Missing secret loud: `:126-146` `load_capability` — `None` → `Err`; wrong length → `anyhow!`.
  Zero `.ok()` / `unwrap_or*` / `_ =>` in the file.
- No secret interpolation anywhere; redacted `Debug` for `CapabilitySecret`
  (share_enrollment.rs:139-146), `OwnerIdentityKey`, `RecoveryCode`, each pinned by a test.
- Sidecar genuinely ed25519 owner-signed: sign `roster_sidecar.rs:105`, real verify `:148-155`
  (`ed25519_dalek` verify, owner_identity/mod.rs:117).
- Leak sweep CLEAN: only ≥32-char literals in `lane-logs/` + report are labelled sha256
  baselines (models-sha256-baseline.txt:1; report lines 21,22,184,186). No secret values.
- **REFUTED sub-claim**: "a share whose roster cannot be rebuilt is NOT advertised
  (`RehydrationFailed` disclosed) — find the branch and its test." The BRANCH exists
  (loro_share_backend.rs:2599-2615, `roster = None`, advertise block :2621-2651 runs only
  `if let Some(roster)`). **No test covers it.** Nothing in `crates/**` asserts the
  not-advertised outcome or the emitted `RehydrationFailed`.

## C3 — revocation: **PARTIALLY REFUTED**
All three legs present (loro_share_backend.rs:1217-1247): un-pin `:1230`, close window
`:1231` + persist `:1234`, forget addrs `:1238`→`:1063`.
- Refused-import test EXISTS: `revoking_a_peer_stops_every_further_import_from_it`
  (loro_share_backend.rs:4428) — asserts `synced == 0` and no post-revocation op enters A.
- **"not re-dialed on restart" test DOES NOT EXIST.** No test combines `revoke_share_peer`
  with `rehydrate_shared_trees`. This leaves the warn-only failure path at `:1072-1079`
  unpinned: if the sidecar rewrite fails, `forget_peer_addrs` only `warn!`s
  ("the revoked peer's addr may come back after a restart") — a revoked peer's addr can
  resurrect across restart with no test catching it.

## C4 — red-first honesty: **REFUTED**
`lane-logs/red.log` has 4 reds; 2 ARE liveness timeouts ("enrollment accept stream timed out
after 10s", at :4508 and :4445) as disclosed. But the STRANGER red is not what is claimed.
Panic: `loro_share_backend.rs:4393:18: gated share` — that is the
`.expect("gated share")` on `roster_for(...)` inside the assert_eq at 4383-4394.
The two headline assertions sit EARLIER and therefore **passed in the red run**:
```
4375: assert!(
4376:     !format!("{:?}", stranger_doc.get_deep_value()).contains("confidential-payload"),
4377:     "an un-rostered peer must not receive the shared subtree"
4379: assert!(
4380:     !shared_doc_debug(&backend_a, &shared_tree_id).contains("stranger-graffiti"),
4381:     "an un-rostered peer's ops must not enter the shared replica"
```
So pre-fix the stranger already leaked NOTHING (the `sharing-admit` base at 5644b6d3 gates
peer IMPORT). The red fired only on a STRUCTURAL probe — "is there a roster at all" — not on
the leak. The lane's framing ("the red fired on the share is gated") is literally true but
materially overstates it: the confidentiality property was never demonstrated red.

**C4 control strength: CONFIRMED (not vacuous).** Both legs are in one test fn, one run,
same `alpn`/live share, and there is an up-front non-vacuity assert that the payload really
is in the share (4331-4335). Control at 4396-4416:
```
4404: sync_doc_initiate_enrolled(&holder_ep, &holder_doc, &alpn, ticket.addr.clone(),
        &ticket.capability, &shared_tree_id, Capabilities::read_write())
4412:   .expect("a peer holding the ticket's capability must enroll and sync");
4413: assert!(format!("{:?}", holder_doc.get_deep_value()).contains("confidential-payload"),
4414:   "control: the capability holder must receive what the stranger was denied");
```
Nit: control runs AFTER the stranger dial, not "at the same moment" as the report says —
but that ordering is the safe direction (a share killed by the stranger's dial would fail
the control).

## C5 — bearer-ticket disclosure: **CONFIRMED**
Typed `ShareDegradedReason::BearerTicketEnrollment { peer }` (degraded_signal_bus.rs:217,
kind const :239); raised iroh_advertiser.rs:360; `warn!` fires even with no bus (:143).
GPUI banner real: `frontends/gpui/src/share_ui.rs:477` → `DegradedKind::BearerTicketEnrollment`
(:205), rendered :2083. Exactly-once test `a_bearer_ticket_admission_is_disclosed_once_on_
the_degraded_bus` (iroh_advertiser.rs:677, assert :746-758) — was red pre-fix and PASSED in
my own run.

## C6 — owner-key recovery code: **GAP (as suspected)**
`share_credentials.rs:105-111` — on first mint, a `warn!` says the one-time recovery code was
NOT shown. The code itself is never logged/printed (verified: only `secret.as_bytes()` reaches
the keychain; `RecoveryCode` has a redacted `Debug`, recovery.rs:65-73, test :227).
**It is LOG-ONLY.** No `ShareDegradedReason` variant, no banner, no user surface. Per the
brief's own criterion this is a GAP: the user loses the ability to verify their roster
sidecars if the keychain entry is lost, and learns it only from a log line.

## C7 — gates: **CONFIRMED (all reproduced by me)**
| Gate | Result |
|---|---|
| `nextest -p holon-sharing -p holon-loro -p holon-loro-wiring` | **474 run, 474 passed, 3 skipped** — matches the claimed 474 exactly |
| `cargo check --workspace --all-targets` | exit 0 |
| same + `--features holon-integration-tests/pbt,holon-gpui/pbt` | exit 0 |
| `just keystone-smoke` + `scripts/keystone-known-reds.sh` | GREEN; known-reds PASS, nothing to classify |
| two-instance composed PBT, `timeout 900`, isolated | **2/2 passed, 212.5 s** (wall 227 s; lane claimed ~530 s — faster here, same 2/2) |
Logs: `…/scratchpad/verify-lifecycle/v-{nextest,check,check-pbt,keystone,knownreds,twoinstance}.log`

## Overall: CONFIRMED WITH DEFECTS
The security substance holds — no production ungated share, real keychain, real ed25519
sidecar signing, loud errors, no secret leakage, revocation works, gates green and
independently reproduced. Three defects and two gaps below.

### Defects (evidence only, not fixed)
D1. `AdmittedPeer::ungated`, `sync_doc_accept`, `sync_doc_initiate`, `start_share_ungated`
    are `pub` and re-exported from `crates/holon-loro/src/lib.rs` → any other crate can
    bypass the H5 gate. Prior verifier flagged this; still unfixed.
D2. Stranger test's red was `.expect("gated share")`, NOT the confidentiality assertions
    (which passed un-gated). Red-for-the-right-reason claim overstated.
D3. `forget_peer_addrs` sidecar-write failure is warn-only (loro_share_backend.rs:1072-1079):
    a revoked peer's dial addr can come back after restart, untested.

### Gaps
G1. No test for the `RehydrationFailed` / share-NOT-advertised branch (the fail-closed
    direction the report calls out as new).
G2. No revoked-peer-across-restart test.
G3. Owner-key recovery-code "not shown" is log-only; expected shape is a typed degraded
    signal + banner.

### Note
`jj diff` in the `sharing-admit` workspace auto-snapshotted its working copy
(`Updated working copy to fresh commit 8da5bef26235`) — jj's normal read-path behaviour,
no content changed, but that workspace's `@` id moved.

---

# Rev 2 delta — re-verification

pwd for every verdict line: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/share-lifecycle`

## (a) Cross-crate reachability: **CONFIRMED CLOSED**
Verified visibility in the tree, not from the report:
- `ShareAdmission::Ungated` — `#[cfg(test)]` (iroh_advertiser.rs:85). **Stronger than briefed**:
  not `test-helpers`, so even a `test-helpers` build of another crate cannot name it. Its
  accept_loop match arm is `#[cfg(test)]` too (:445-450).
- `start_share_ungated` — `#[cfg(test)] pub(crate)` (:156-157).
- `AdmittedPeer::ungated` — `#[cfg(any(test, feature="test-helpers"))] pub(crate)`
  (peer_import.rs:94-95). `pub(crate)` alone makes it cross-crate unreachable regardless of feature.
- `sync_doc_accept` — `#[cfg(any(test,test-helpers))] pub(crate)` (iroh_sync_adapter.rs:381-382);
  **re-export deleted**, documented at :1137-1139.
- `sync_doc_initiate` — same cfg + `pub(crate)`; re-export narrowed to
  `#[cfg(all(…, test))] pub(crate) use` (:1145-1146).
- `IrohSync` — stays `pub` but under `#[cfg(any(test, feature="test-helpers"))]` (:550-563).

**What `test-helpers` exposes**: `holon-loro/Cargo.toml:16 test-helpers = ["dep:proptest",
"dep:proptest-state-machine"]`; it un-cfgs `pub mod multi_peer` (lib.rs:82-83), `IrohSync`, and
the `pub(crate)` items above (which stay crate-private anyway). The only cross-crate reach is
`holon_loro::iroh_sync_adapter::IrohSync` from `crates/holon/tests/sync_suite/sync_pbt.rs:76,94`.
**Which crates enable it — all dev/test, none production**: holon-app `[dev-dependencies]`:44-45;
holon `[dev-dependencies]`:96,104; holon-integration-tests:81,108; holon-loro-testing:28;
holon-mcp-mock:32; holon-loro-wiring:17 (feature passthrough). No production dependency edge.

**Compiler proof, reproduced by me**: `cargo check -p holon-loro --lib` (no `test`, no
`test-helpers`) exit 0 — the ungated surface is absent from the shipping build, and the only
5 warnings are pre-existing `holon-filesystem` `IngestOutcome` ones, none from `holon-loro`
(report's no-warnings claim CONFIRMED). `cargo check --workspace --all-targets` exit 0, and
again with `--features holon-integration-tests/pbt,holon-gpui/pbt` exit 0.
The lane's inversion probe is a genuine build failure, not a test failure:
`lane-logs/r2-red-attempt1-ungated-is-uncallable.log` → `error[E0599]: no variant named
'Ungated' found for enum 'ShareAdmission'` … `could not compile holon-loro (lib)`.

## (b) Tests: **CONFIRMED (480) — but a NEW flake found**
- Six new tests exist and were red for the right reason (`lane-logs/r2-newtests-red.log`:
  `6 tests run: 0 passed, 6 failed`): `a_revoked_peer_is_not_re_dialed_after_a_restart`,
  `a_share_whose_roster_cannot_be_rebuilt_is_not_advertised`,
  `a_revocation_that_cannot_be_persisted_fails_loudly_naming_the_peer`,
  `the_dropped_owner_recovery_code_is_disclosed_once_on_the_first_share`,
  `minting_the_owner_key_discloses_that_its_recovery_code_went_unshown`,
  `the_recovery_code_never_reaches_the_bus_or_a_log`. My G1/G2/G3 gaps are closed.
- `cargo nextest run -p holon-sharing -p holon-loro -p holon-loro-wiring`, four full-suite runs
  by me: **run 1 = 480 run, 479 passed, 1 FAILED**; runs 2, 3, 4 = **480 passed, 3 skipped**.
- `just keystone-smoke` ok (4 passed) + known-reds PASS. Two-instance PBT **2/2, 170 s**
  (wall 185 s; lane claimed 516 s — both 2/2).

### R2-D1 (NEW DEFECT) — `revoking_a_peer_stops_every_further_import_from_it` is load-flaky
Reproduced once in 4 full-suite runs; **6/6 PASS in isolation**, so it is contention-dependent.
`loro_share_backend.rs:4480` unwrap on:
```
"the peers sidecar for share e7d7c8bb-… could not be rewritten after revoking peer
 PeerFingerprint(f7f5534a…), so that peer's dial addr survives on disk and comes back at the
 next restart: rename …/shares/<id>.peers.json.tmp → …/shares/<id>.peers.json:
 No such file or directory (os error 2)"
```
`shared_snapshot_store.rs:151 save_peers` does `create_dir_all(&self.shares_dir)` before the
write, so ENOENT at the **rename** means the shares dir disappeared between create and rename
under parallel load. This is the very path rev 2 made loud (defect D3 fix), so the flake sits on
a security-relevant branch: in production this returns `Err` from `revoke_share_peer`, i.e. a
revocation that reports failure while the peer stays dialable. Per CLAUDE.md this
load-sensitive gate red should get a bugfunnel entry. Not fixed by me.
Evidence: `…/scratchpad/verify-lifecycle/r2-nextest.log` (the red), `r2s-{1,2,3}.log` (green),
`r2b-revoke-{1..6}.log` (isolated green).

## (c) Recovery code: **CONFIRMED**
- `ShareDegradedReason::OwnerRecoveryCodeNotShown` is a **unit variant**
  (degraded_signal_bus.rs:237) — structurally cannot carry the code; kind const + arm at :290.
- `ShareCredentials::owner_key(&self, degraded_bus: &DegradedSignalBus)` (share_credentials.rs:109)
  takes the bus as a REQUIRED argument, so no caller can mint a key with nowhere to disclose;
  emit at :122. Callers updated (loro_share_backend.rs:1142, 1170, 1208).
- GPUI banner real: `frontends/gpui/src/share_ui.rs:495` → `DegradedKind::OwnerRecoveryCodeNotShown`
  (:210), style/copy arm :2109.
- Never logged/rendered, pinned by exact-`Debug` equality rather than a substring probe
  (share_credentials.rs:291-297) plus `RecoveryCode`'s own redacting `Debug` (:298-300).
- Fires once: `minting_the_owner_key_discloses_…` calls `owner_key` twice (:253-254) and
  `the_dropped_owner_recovery_code_is_disclosed_once_on_the_first_share` pins it through
  `share_subtree`. Both green in my runs.

## (d) Honesty rewrite: **CONFIRMED — matches the logs**
Report §Rev 2 · 4 (lines 578-586) states the rev-1 stranger red was a panic at
`.expect("gated share")`, a structural probe, and that its two confidentiality assertions
PASSED because the `sharing-admit` base already gates peer import — therefore the
confidentiality property **cannot be shown red on this base**. That is precisely and
independently what I established in rev 1 (red.log panic `4393:18: gated share`; asserts at
4376/4380 sit earlier). Nothing overstated; no red manufactured. The probe restore is
also honest — inverted-guard probes produced a genuine 6/6 red and the two probed files were
restored (sha pair `r2-probe-green-sha.txt` == `r2-probe-restored-sha.txt`). The disclosed
sccache infrastructure failure (§Rev 2 gates, lines 640-645) is consistent with the logs.

## Rev 2 verdict: **CONFIRMED, with one new defect (R2-D1)**
All five rev-1 defects/gaps (D1, D2, D3, G1, G2, G3) are genuinely closed, and D1 is closed
by construction — the compiler, not a grep. The one new finding is the load-sensitive flake
R2-D1 above; it does not weaken the security boundary, but the 480-green claim holds only
per-run, not reliably, and that should be recorded before landing.


## Rev 3 — adversarial verification (fresh context)

pwd for every verdict line: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/share-lifecycle`

### C1 — cause: **CONFIRMED, with one correction**
- Pre-fix `peers_tmp_path` IS a pure function of the share id:
  `jj file show -r @- crates/holon-loro/src/shared_snapshot_store.rs:96-99` →
  `shares_dir.join(format!("{shared_tree_id}.peers.json.tmp"))`. Same shape for
  `tmp_path` (:84), `.port.tmp` (:198), `.gen.tmp` (:261).
- Pre-fix `remember_peer` snapshots the map, RELEASES the guard at the block end
  (prefix backend :1002) and only then calls `save_peers` (:1003). Confirmed.
- **Correction**: at `@-` (830d794f) `forget_peer_addrs` does not exist — the ONLY
  `save_peers` caller in history is `remember_peer:1003`. The revocation writer is
  itself a rev-2 (uncommitted) addition. The "two production writers" pair is real
  in the tree under review, but it is not a pre-existing-on-main pair. The
  mechanism is still pre-existing on main: `remember_peer` runs in a
  `tokio::spawn`ed task per admitted dial, so two admissions for one share already
  raced the same fixed tmp before this lane.
- No code removes the shares dir: `rg 'remove_dir' crates/ frontends/` = 4 hits
  (`consolidator_epoch.rs:161`, `pairing_swap.rs:141` = pairing staging,
  `holon-worker/patches/symlink`, an orgmode test). None touches `shares/`.
  The earlier hypothesis is REFUTED, as the lane says.
- I reproduced the POSIX mechanism independently (python, two threads, one fixed
  tmp name, slow writer stalled 400 ms): `{fast: 'ok', slow: FileNotFoundError(2)}`
  and the published file holds the FAST writer's 10 bytes, not the slow writer's
  100. Same signature as R2-D1.

### C2 — fix halves: **CONFIRMED**
- Four publish paths, all through `stage_tmp`/`publish_tmp`:
  `save` (shared_snapshot_store.rs:160/169), `save_peers` (:192/203),
  `save_port` (:234/235), `save_generation` (:285/286). `stage_tmp:119-138` builds
  `<final name>.<pid>-<seq>.tmp` from `std::process::id()` + a static `AtomicU64`.
  No fixed `.tmp` name construction survives in the file (`rg '\.tmp'` → only the
  nonce format string, the sweep suffix check, doc comments and test fixtures).
- Both peer writers persist UNDER the `known_peers` write guard:
  `loro_share_backend.rs:1024-1047` (`remember_peer`, `save_peers` inside the
  block) and `:1081-1088` (`forget_peer_addrs`).
- **Every `save_peers` caller is one of those two** (`rg save_peers crates/
  frontends/` → 1047, 1088, plus tests/comments). No writer persists outside the
  guard.
- `lane-logs/r3-red-2-fixA-only.log` says exactly what is claimed: with the unique
  tmp name only, `7 tests run: 6 passed, 1 failed` — the failure is
  `a_concurrent_admission_cannot_restore_a_revoked_peers_addr_on_disk` panicking
  "a revoked peer's dial addr must not survive in the sidecar: [EndpointAddr {
  id: PublicKey(3268ce1d…) }]", while
  `a_concurrent_admission_does_not_fail_a_revocations_sidecar_write` PASSES — i.e.
  the loud ENOENT is gone and the lost update is silent, both calls `Ok`.

### C3 — tests: **CONFIRMED**
- 4 new tests, all present and non-trivial:
  `a_concurrent_peer_save_does_not_steal_this_writers_tmp_file` (store:518),
  `concurrent_snapshot_saves_publish_a_loadable_file` (store:551),
  `a_concurrent_admission_does_not_fail_a_revocations_sidecar_write` (backend:4637),
  `a_concurrent_admission_cannot_restore_a_revoked_peers_addr_on_disk` (backend:4668).
  The last one genuinely depends on half B: the stalled `remember_peer` holds the
  write guard for 500 ms, so `forget_peer_addrs` can only land after it.
- Red logs match their claims: `r3-red-1.log` = `5 passed, 2 failed` with the
  verbatim production ENOENT; `r3-red-3-save-probe.log` = `2 passed, 1 failed` on
  the snapshot path; `r3-green-1.log` = `7 passed`.
- Probe restore: `r3-probe-{green,restored}-sha.txt` both
  `92710315ff15fea3…`. NOTE: the file's sha today is
  `d99350ab66e647f1…` — later legitimate edits, not probe residue; I checked the
  current `stage_tmp` carries the nonce and no fixed-name path remains.
- `2026-09-02-shared-snapshot-tmp-path-torn-write` flip OPEN→FIXED is JUSTIFIED:
  the entry names the identical mechanism (one `<id>.loro.tmp` for four
  unsynchronised writers) and lists "a per-writer unique temp name" as one of its
  two acceptable remedies; `stage_tmp` is that remedy, the covering test is named
  (`concurrent_snapshot_saves_publish_a_loadable_file`), and the residual PBT
  oracle gap is recorded rather than dropped.
- `bugfunnel.py check` → `658 entries, 0 problems` (my run).
- Suite run TWICE via `with-build-slot.sh` on my own script
  (`…/scratchpad/v3/gate.sh`, tree asserted by `grep -q 'fn stage_tmp'`):
  `484 tests run: 484 passed, 3 skipped` / `SUITE1_EXIT=0` and the identical line
  with `SUITE2_EXIT=0`
  (`…/scratchpad/v3/v3-suite-{1,2}.log:552-553`).
- Extra stability probe of my own: the 6 race/revocation tests run 8 consecutive
  times, `6 passed` every time (`…/scratchpad/v3/repeat-{1..8}.log`). R2-D1 did
  not recur.

### C4 — recorded-not-fixed: **CONFIRMED**
- `next_generation` (store:292-296) is `load_generation + 1` then `save_generation`
  with no lock. It IS filed — lane report §6 item 5. Severity, one sentence: two
  *different* peers can never collide because `stable_peer_id` mixes the device
  key, so the real (low-likelihood, correctness-relevant) hazard is one device
  concurrently reaching `accept_shared_subtree` twice for the same
  `shared_tree_id` and reusing its OWN previous Loro peer id — an op-id collision
  on the shared doc rather than a cross-peer identity clash; the only two callers
  are `loro_share_backend.rs:1808` (`share_subtree`, which mints a fresh id, so it
  cannot collide) and `:2002` (`accept_shared_subtree`).
- `revoke_share_peer` has no production caller: `rg revoke_share_peer crates/
  frontends/ docs/` → the definition (`:1236`), three test call sites
  (`:4488`, `:4543`, `:4610`) and two prose mentions in bugfunnel entries. No GPUI
  action, no MCP op. CONFIRMED.

### Residual observations (no defect claimed, no repro)
1. `sync_pbt`'s `scan_for_corruption` widening to `.tmp` also starts seeing
   `.port.tmp`/`.gen.tmp`, but P-NO-TMP-LEFTOVER retries for 30 s after the first
   settle (`sync_pbt.rs:1053-1085`), so a transient tmp cannot flake it.
2. `publish_tmp` swallows both the directory-open failure and the directory fsync
   error (`if let Ok(dir) … let _ = dir.sync_all()`). A rename that is not
   durably linked is then reported as a successful publish. Pre-existing shape,
   not introduced here, and out of R2-D1's scope.
3. Test seams are properly `#[cfg(test)] pub(crate)`
   (`stall_next_peers_publish`, `peers_publish_stall_once`) — no production reach.

### Rev 3 verdict: **CONFIRMED**
All four claims hold against evidence I produced myself. R2-D1's root cause is
correctly identified (and the shares-dir hypothesis correctly refuted), both fix
halves are in place at every publish path and every sidecar writer, the
fix-A-only log proves half B is required rather than belt-and-braces, the
bugfunnel flip is justified, and the suite is 484/484 twice plus 8/8 on the
race tests. The one correction to the lane's wording: at `@-` the second peer
writer did not yet exist, so the racing PAIR is a rev-2 artefact — the racing
MECHANISM (two spawned `remember_peer` tasks on one fixed tmp) was already on
main.
