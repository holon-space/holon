# `just hand-authored` red at wave-11 tip 91b1501d — attribution

Verdict: **NOT attributable to lowcode-inc3. The red is a load-sensitive wall-clock FLAKE.**
Tree asserted: `jj -R /Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ log -r '@-'` = `mxqvlkvw 91b1501d integration* sw/lowcode-inc3`; pwd `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ` (only the pre-existing `M frontends/holon-worker/Cargo.lock` dirt).

## Runs

| Run | Tree | Result | Failing test | Assertion | Wall |
|---|---|---|---|---|---|
| A (given) `scratchpad/w-quick-open-contrast-hand.2389.log` | tip 91b1501d | FAILED 8/9 | `hand_authored_keystone_regressions`, case `journals-external-rewrite-strips-convert-link-marks`, transition 10/22 `CreateBlockUnderFocus` | `components.rs:3913` → `user_driver.rs:722` "creation-affordance birth … did not land within 3s" | 517 s |
| B (mine) `scratchpad/verify-tip-rerun.log` | tip 91b1501d, same workspace | FAILED 8/9 | `echo_loop_block_to_page_child_render_leak_parked` | `wide_e2e.rs:979` "[boot journal] auto-create rule did not fire journal … within budget" (10 s) | 1898 s |

Cross-check: in run B the run-A culprit case PASSED (`verify-tip-rerun.log:2443`); in run A the run-B test passed (`…2389.log:375`, `test echo_loop_… ok`). Two runs, same binary, disjoint failures → **not deterministic**. Bisect to `ec6ae4c7` was therefore skipped: a green there would be evidence-free.

## Mechanism

Both failures are fixed wall-clock deadlines, not oracle divergences:
- `crates/holon-frontend/src/user_driver.rs:722` — 3 s poll for the newborn row after the focus-edge `block.create`.
- `crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:971-985` — 10 s poll for the boot journal.

Corroborating load evidence in run A right before the panic: `holon_latency … stage="e2e_expired" action=split_block waited_ms=31420 / 31640` and repeated `over the p95 SLO but within the machine-load slack`. Run B took 3.7× run A's wall time on the same tree — the box is contended (other lanes building).

Residual, not excluded: lowcode-inc3 adds SQL work on the ingest path (`persist_file_projection` now UPSERTs the `file` row for every ingest, `crates/holon-filesystem/src/sync_ports.rs:122-141`, `file_sync_controller.rs:4554-4600`) and a `debug!` per cold-boot-skip candidate. That could shave margin off these deadlines but cannot by itself produce a deterministic red, and the FileSyncController "write-back SKIPPED" warning seen around the panic pre-exists the commit (present at `ec6ae4c7`, `file_sync_controller.rs:5160`).

## Smoke consistency

Consistent. The keystone smoke exercises short cases; both flaky assertions need a long, contended case (transition 10 of 22, or a full frontend boot under load) to exceed a 3 s / 10 s budget. Smoke has no oracle these two would violate.

## Recommendation (not applied — verifier does not fix)

Do not attribute this to lowcode-inc3 and do not revert. If wave-11 landing needs a green gate, either serialize the run (no concurrent cold builds) or file the two deadlines as flake-prone budgets (bugfunnel: environment gap).
