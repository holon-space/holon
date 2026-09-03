# Handoff — 2026-09-07/08 (orchestrator session bc7b1e67, wave 9 LANDED, wave 10 starting)

Resume from THIS file plus `followup-queue.md` (same directory; the running log, newest at the bottom). Do not reread session transcripts.
Verify every load-bearing claim with the check command given before acting on it.

## 0. Where the code is

| Thing | Where | Check |
|---|---|---|
| main | `34fca6bf0b8d` (= `main@origin`, wave 9 = 12 commits) | `jj log -r main@origin -T commit_id` |
| integration chain (wave 10) | `main@origin..integration` = pair-conflict-badge `49694088` (change `qwmzlsqm`, bookmark `sw/pair-conflict-badge`), no CONFLICT | `jj log -r 'main@origin..integration' -T 'if(conflict,"CONFLICT ","ok ") ++ commit_id.short() ++ " " ++ description.first_line() ++ "\n"'` |
| integration workspace | `.claude/worktrees/_sw_integ` | `jj workspace list \| grep _sw_integ` |
| lane workspaces | `.claude/worktrees/<lane>/` (jj workspaces) | `jj workspace list` |
| this bundle | `docs/Handoff/2026-09-08/` | `ls docs/Handoff/2026-09-08` |
| verifier verdicts + lane reports | `docs/Handoff/2026-09-08/reports/` | `ls docs/Handoff/2026-09-08/reports` |
| gate scripts as they ran | `weave-gate.sh`, `land-gate.sh`, `land-when-quiet.sh`, `land-check.sh`, `weave-msgs.md` in this directory | `ls docs/Handoff/2026-09-08/*.sh` |
| previous bundle | `docs/Handoff/2026-09-03/HANDOFF-afternoon.md` | — |

## 1. Rulings

Applied this session (memory `rulings-2026-09-07-d93-d94`):

- **D93.a** — a pairing conflict copy (`pairing_conflict_of=<id>`) renders as a visible badge through a `block_profile` variant, plus the pairing disclosure listing the count and a query link. Never mark by title. → lane `pair-conflict-badge`, rev 2 CONFIRMED, woven as wave 10 commit 1.
- **D94.a** — an unsatisfiable re-import at boot (`ReimportHasNoParent`) boots DEGRADED: skip the owed re-import, sticky banner naming the archive path and the count, keep the marker so the next boot retries, add a "retry re-import" action; the `.expect()` stop-at-boot is removed. → lane `pair-boot-degraded`, rev 2 CONFIRMED, unwoven.

OPEN for Martin:

- **D95** — retry re-import on the banner (as built, D94 deviation) vs a Settings action needing a new `device` type profile; rec (a) keep the banner. Posted to the decision inbox 2026-09-08 04:20.
- **V1–V12** — the Fable vision review questions, still unanswered (carried from the 09-03 bundle, `docs/Handoff/2026-09-03/vision-review.md`).

## 2. Lanes

### Landed in wave 9 (`main` 4e2ee368 → 34fca6bf, 12 commits)

Check any row with `jj log -r <id> -T description.first_line()`.

| Lane | Commit | Note |
|---|---|---|
| search-fix (rev 3) | `f86d68bc` | full Unicode fold classes, typed GLOB length refusal; cmd-K dogfood PASSED |
| lowcode Inc 2 (plugin host) | `438eef0a` | wasmi guest host + cooklang guest |
| lowcode Inc 4 (mapping) | `0af4c041` | UTCP manual + `holon:` section, RowMapper; incl. the holon-mcp-mock consumer fix |
| pair-reimport (D78.d) | `a47d8de2` | crash-safe pairing swap |
| readonly-edits | `3f62a9e9` | read-only-format cell refuses and discloses |
| sql-loss | `093a6385` | cascade-swept vs sink-loss diagnostic |
| target-gc (D85.c) | `0b4bf6c4` | `scripts/target-gc.py`, CANON gate normalisation (`build` kept off CANON) |
| docs-truth | `46e67d82` | invariant 13, CrateMap regenerated, `identity_minting` archlint |
| dogfood-search | `d2ed2537` | 9 bugfunnel entries + cmd-K re-run sections |
| guests-repro | `5c2db13e` | reproducible guest wasm (fixed stage path, remap, `--locked`, no wasm-opt) |
| mcp-authority-fix (rev 3) | `cf54fe23` | pre-existing boot panic for read-only sidecar mirrors + remote-discovery leg; typed `MirrorSchema` |
| smoke-ab docs | `34fca6bf` | 2 ENVIRONMENT bugfunnel entries for the load-induced novel reds |

### Wave 10 seeds

| Lane | WS | State | Next step |
|---|---|---|---|
| pair-conflict-badge (D93.a) | `.claude/worktrees/pair-conflict-badge` | rev 2 CONFIRMED 6/6, no defects (`reports/pair-conflict-badge-verify-r2.md`; rev 1 verdict `…-verify.md`; lane report `reports/pair-conflict-badge/`). **WOVEN** as `49694088` = current `integration`. Gaps: Page windowed test runs against a quarantined ingest; the count→badge seam is uncovered; a rule-head conflict copy is not org-seedable. | Run `bash weave-gate.sh pair-conflict-badge` at the tip from `_sw_integ` if it has not been run at `49694088`, then weave `pair-boot-degraded`. |
| pair-boot-degraded (D94.a) | `.claude/worktrees/pair-boot-degraded` | rev 2 CONFIRMED 5/5, click #2 proved through the button (`reports/pair-boot-degraded-verify-r2.md`; rev 1 `…-verify.md`; lane report `reports/pair-boot-degraded/`). NOT woven. Its base is mid-chain — rebase onto the tip first. | `sw weave pair-boot-degraded -m "<msg from weave-msgs.md>" --check "bash weave-gate.sh pair-boot-degraded"`, then the quiet land gate for wave 10. |

### Queued (not started; anchors from the log)

- **quick-open-focus-fix** (P1) — dead keyboard after Escape closes quick-open; still reproduces in the 2026-09-08 cmd-K dogfood; the invariant `inv-window-focus-matches-engine-focus` reports *skipped* live, so make it actually run. WS `.claude/worktrees/quick-open-focus`.
- **quick-open latency stage** — quick-open emits no `holon_latency` stage, so the p95 SLO is unmeasurable (MCP round-trip floor measured p95 733 ms).
- **clippy-reds** — `just lint` RED on pre-existing holon-api: 54 `double_must_use` (from `#[async_trait]`) + 1 `large_enum_variant` (`entity.rs:118`); `deny` and `machete` also fail. Not in the landing gate, so not a wave blocker; suspected exposed by the 08-16 toolchain bump.
- **delete `test_turso_backend_state_machine`** — D65.a says DELETED; it is still on main and red.
- **G2 mirror-table reconcile** — `QueryableCache::initialize_schema` never reconciles an existing mirror table with a changed sidecar schema, so a vault predating `cf54fe23` meets `no such column: properties` at write time.
- **holon-mcp `describe_ui_deferred` ×2** — pre-existing reds; the fixture omits `integration_state.display_name`.
- **upstream sql-loss `parent_id`** — `crates/holon-loro/src/loro_sync_controller.rs` `block_diff_params` emits `parent_id` only when old≠new, so the receiver's reseed stays load-bearing.
- **pinblock-render**, **driver-titlebar**, **rules-unparsed** (WS empty) · **admit-routing** (D86.a, security-executor) · **visual-accent** (D87.a) · **lowcode Inc 3** (D90.a + D91.a: delete `cook.rs`, incremental content-hash scan, ADVANCED_UNITS off) · **D88.a device list follow-up** · **dogfood-explorer rotation** (Martin's standing order: settings → rules → newest feature → visual).
- **guests-repro follow-up** — shared stage root concurrency was closed with an mkdir lock; the bounded root sweep (14 days) is new and unexercised.
- **Signal ping queued** (quiet hours at land time): "Wave 9 landed (12 commits, main 34fca6bf); D95 open in the inbox".

## 3. Landing recipe as it actually ran

1. **Per-lane weave gate.** `sw weave <lane> -m "<message from weave-msgs.md>" --check "bash weave-gate.sh <lane>"`, run from `_sw_integ`. Each case in `weave-gate.sh` starts with a **sentinel grep** (`WRONG TREE` → exit 9), then a crate list, then optional extras (`hand`, `loro`, bugfunnel check). Crate lists must include **consumer** crates (holon-mcp-mock, holon-loro-wiring) — omitting them let a break reach the `--workspace` check late.
2. **Re-gate at the tip after every rebase or conflict resolution.** Every wave-9 commit was gated at the tip it ended on, not at the tip it was written against.
3. **Quiet wrapper.** `land-when-quiet.sh` polls `sysctl -n vm.loadavg` and `pgrep -x rustc` every 120 s and runs `land-gate.sh` only at `load1 < 10 && rustc < 3`. It needed `(pgrep -x rustc || true)` — under `set -euo pipefail`, `pgrep` exit 1 at rustc=0 killed the loop. Launch it nohup-detached with TERM/HUP ignored.
4. **Land gate.** `land-gate.sh`: four sentinel greps + a token-shaped-segment scan over `crates docs assets`, then `just landing-gate` under `sem --id holon-build -j4` in parallel with `cargo nextest run --no-fail-fast -p holon -p holon-app` (D43.a/D64.a). Allowlist (regex, replacing the old comm file): `e2e_backend_engine_test`, `undo_concurrent_keystrokes`, `test_multi_peer_sync_iroh`, `turso_block_query_source_round_trip_pbt`, `cursor_filtered_main_panel_delivers_at_vault_scale`. Anything else = NOVEL and blocks.
5. **Binding land check.** `land-check.sh` runs at land time and refuses unless the tree is still the measured one: `@-` = `34fca6bf0b8d`, `MirrorSchema` present in `crates/holon-mcp-client/src/mcp_sidecar.rs`, a `== landing gate PASS ==` marker in the newest battery log, the gate log naming the right run (`quiet at 03:48` in `land-gate-wave9-r5.log`), and exactly `725 tests run` in the app nextest summary. Its allowlist is narrower than the gate's: `e2e_backend_engine_test`, `test_multi_peer_sync_iroh`, `test_turso_backend_state_machine`.
6. **Land.** `sw land --push`, confirm `main@origin` moved, `jj new main` in the session repo and every landed lane WS, purge landed lanes' `target/`.

## 4. Process facts learned (HAZARD / PATTERN lines from the log)

- **HAZARD** — never run `sw new --base integration` while a weave is moving `integration`: `pair-boot-degraded` was based on a conflicted tip (healed by the squash + `update-stale`).
- **HAZARD** — `land-when-quiet.sh` pipefail bug: `pgrep` exit 1 under `set -e` killed the loop; fixed with `|| true`.
- **HAZARD** — killing the quiet wrapper leaves `just landing-gate` running; sweep the pid tree and the semaphore, and treat its result as invalid if the tree was rewritten meanwhile.
- **HAZARD** — weave-gate crate lists that omit consumer crates catch breaks only at the `--workspace` check.
- **PATTERN** — agents stall on network blips at their first tool round; `SendMessage` to the agentId resumes them (three at once on 2026-09-08 04:40).
- **PATTERN** — novel keystone reds under load ≥50 need a **population A/B**, not a rerun: 10× tip vs 10× parent showed both 10/10 no-novel, proving the two novel signatures (SutOrgRender `structural-page.org` NotFound; inv-sql-budget divergence) were load-induced at load 57 / 12 rustc.
- **PATTERN** — iroh transport tests (`loro_share_backend` accept, gated-share ticket) flake under cross-lane build contention and pass 3/3 in isolation; both this and the keystone flake are now bugfunnel ENVIRONMENT entries in `34fca6bf`.
- **PATTERN** — a rewrite of a mid-chain commit rebases every descendant; hold any such squash until the live weave/gate on the chain finishes.
- **PATTERN** — a dogfood agent that greps only its own lane tree files duplicate bugfunnel entries; merge into the owning docs lane instead.
- A lane may deviate from a ruling with a stated reason — D94 put retry on the banner because Settings only carries `PreferenceDef` fields — and the deviation becomes a decision (D95), not a silent change.

## 5. What is pushed

`main` = `34fca6bf` on origin (wave 9, confirmed remote at 15:40). `integration` and `sw/pair-conflict-badge` sit at `49694088` locally — **not** confirmed pushed; check with `git ls-remote origin integration 'sw/*' 'wip/*'`. `pair-boot-degraded` is unwoven and lives only in its workspace. This bundle itself is written into the `handoff` workspace and is unpushed until the orchestrator commits and pushes it.
