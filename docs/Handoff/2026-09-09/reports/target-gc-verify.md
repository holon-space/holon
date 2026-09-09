# Adversarial verification — lane `target-gc` (D85.c)

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/target-gc`
Identity assert: `scripts/target-gc.py` present, `CANON` in `justfile` — PASS.
`jj status` at end: exactly `M DEVELOPMENT.md`, `M justfile`, `A scripts/target-gc.py`.
Logs: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/target-gc-verify-logs/`

## Overall: CONFIRMED (7 of 8); claim 5 REFUTED-AS-STATED (no regression)

### 1. Self-test has teeth — CONFIRMED
`c1-selftest.log:1-9` — `self-test PASS`, exit 0.
Three mutants of a COPY (real file never edited; sha256 `e3ff652f…578c` identical
`sha-before.log:1` vs post-run recheck):
- `mutant1.py` (sort `reverse=False`) → `c1-mutant1.log`: AssertionError at line 238, exit 1.
- `mutant2.py` (`members[keep+1:]`) → `c1-mutant2.log`: AssertionError line 236 `groups == []`, exit 1.
- `mutant3.py` (drop fingerprint identity, group by hash) → `c1-mutant3.log`: AssertionError line 233, exit 1.
Teeth cover ordering, cut point, and grouping key.

### 2. Busy guard — CONFIRMED
`c2-busy.log`. Scratch fixture `fixture-target`. Idle → exit 0 (line 2 `idle exit=0`).
A compiled binary named `rustc` holding the dir in argv → script printed
`refusing to run: a build is using …fixture-target -- rustc(99658)` and `busy exit=3`.
After it exits → `idle-again exit=0`. No real `target/` touched with `--apply`.
Residual risk (not a refutation): the guard reads argv only. A cargo build whose
target dir arrives via `CARGO_TARGET_DIR` env, or a moment with no live rustc child,
reads as idle. Impact is bounded — the in-use dir is always the newest and is kept.

### 3. Keep newest 2 of 4 — CONFIRMED
`c2-c3-fixture.log:2-11`. 4 generations of `mycrate`/`lib-mycrate`; dry run listed
`g2…`/`g1…` (the older 2) as superseded, `keep g4…`, `keep g3…`, `would free 2.0 KB`,
and all 4 dirs still on disk after the dry run. `--apply` then left exactly
`g3…`,`g4…`; a second pass reported `groups pruned : 0` (idempotent).

### 4. CANON runs the same tests — CONFIRMED (with flagged compile-scope changes)
Method: base `justfile` extracted with
`git -C /Users/martin/Workspaces/pkm/holon archive 4e2ee368 justfile` → 57320 bytes,
non-empty (`base/justfile`). `just -f <file> -d <workspace> -n <recipe>` expanded for
all 71 base recipes into `exp-base/` / `exp-lane/`; diff in `c4-dryrun.log`.
27 recipes differ; 6 of those (`arch-*`) differ only because `justfile_directory()`
resolves to the scratch dir for the base copy — not real differences.
**Test/target selection is byte-identical in every case**: every `--test <name>`,
nextest `-E`, filter arg and env var is preserved (`c4-dryrun.log`, per-recipe diffs).
The only selection edits, both checked:
- `gate-arch`: `-p holon-architecture-tests` → `--workspace … --test architecture_rules`.
  `cargo metadata` shows that package has exactly two targets, `lib` and
  `test architecture_rules`, and `crates/holon-architecture-tests/src/lib.rs` contains
  0 `#[test]` — the running set is unchanged.
- `pbt-lib-slices`: `-p holon-integration-tests --lib` → `--workspace --lib -E
  'package(holon-integration-tests)'` — same test set, selected by filter instead of `-p`.
Every `--test` name used is workspace-unique (`general_e2e_composed_pbt`, `petri_e2e_pbt`,
`round_trip_pbt`, `org_block_round_trip_pbt`, `api_suite`, `hand_authored_regressions`,
`latency_slo_gate`, `loro_suite`, `architecture_rules`, `gpui_gherkin_replay` — each →
exactly 1 package). The only workspace duplicates (`no_ref_state_dep`,
`profile_certification`) are unused by any selector.
`web-arm` is behaviourally inert outside its own targets: the only `cfg(feature="web-arm")`
sites are `crates/holon-integration-tests/tests/web_arm_keystone.rs:41` and
`web_arm_spike.rs:21` (whole-file gates), so turning it on globally adds compile scope,
not test behaviour.
`test` recipe: unchanged, as ruled.
**Flag (not a test-set change, but a real behaviour delta):** `build`, `clippy`, `lint`,
`analyze-clippy` moved from bare `--workspace` to `--workspace --features
holon-integration-tests/pbt,holon-integration-tests/web-arm,holon-gpui/pbt`.
Consequences: `just build` now emits a pbt/web-arm-instrumented app binary, and
`just lint` (`-D warnings`) now lints previously-unlinted pbt-gated code, so it can go
red on code that was green at base. Neither was exercised in this session.

### 5. D64.a (`nextest --no-fail-fast -p holon -p holon-app`) — REFUTED AS STATED, no regression
`grep -n '\-p holon\b' base/justfile` and the lane justfile: **no such invocation exists
in either tree**, and `landing-gate` at base (`base/justfile:1120-1143`) has no nextest
`-p holon -p holon-app` step. `grep -rn holon-app justfile scripts/` in the lane hits only
comments and `scripts/defensive-baseline.txt`. `~/.claude/skills/stacked-workstreams/sw.sh`
contains no `holon-app`/`landing-gate`/`nextest` reference.
So the premise "must STILL run" is false — it was never in the justfile. The lane does not
remove it. `CANON`'s `--workspace` does put both crates in the compile set of every gate,
but no gate RUNS their tests (every runner is narrowed by `--test <name>`), at base or now.
**DEFECT (pre-existing, not introduced): D64.a's per-land `-p holon -p holon-app` test run
is absent from the justfile**; the lane's comment at `justfile:20-22` claims `--workspace`
"satisfies D64.a for free", which is true for compiling but not for running.

### 6. `just target-gc` = step 11, non-fatal — CONFIRMED
`justfile:1174-1175`:
`echo "== landing [11/11]: target-gc (D85.c, this lane's own target/ only) =="` then
`just target-gc || echo "target-gc: non-fatal (busy or nothing to reclaim), see above"`.
Under `set -euo pipefail` the `|| echo` makes a non-zero exit (incl. the exit-3 busy path)
non-fatal. Steps renumbered 1/11–11/11 consistently (`c4-dryrun.log`, `landing-gate` diff).
Recipe body `justfile:1067-1070` runs `/usr/bin/python3 scripts/target-gc.py {{target_dir}}
--apply {{FLAGS}}` (default `target`).

### 7. `just --list` — CONFIRMED
`c7-justlist.log`, exit 0, 78 recipe lines, no parse error. `just --summary` on both trees
shows one added recipe (`target-gc`) and none removed.

### 8. No half-built-artifact damage from the lane's `pkill` — CONFIRMED
`c8-check.log` — `cargo check -p holon-app` under the `holon-build` semaphore,
`RUSTC_WRAPPER=`, `CARGO_BUILD_JOBS=6`: `Finished \`dev\` profile … in 6m 42s`, exit 0,
zero errors/warnings emitted for holon-app.
