# Verify: clippy-reds — OVERALL CONFIRMED (2 defects, 2 gaps)

Workspace `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/clippy-reds`; tree assert OK (async-trait 0.1.92).
Evidence: /private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/verify-clippy/

## C1 CONFIRMED
`jj diff --git Cargo.lock` = 60 lines, 13 +/- lines, nothing else.
async-trait 0.1.91->0.1.92 + checksum. Removals, exactly 7, no [[package]] block vanished:
holon-filesystem: indexmap 2.14.0 | holon-logseq-db: tracing | holon-loro: futures-signals |
holon-loro-wiring: chrono, loro, serde, serde_json. Cargo.toml diffs match 1:1.

## C2 CONFIRMED (with 2 defects)
36 added `#[allow]`, 1 removed (only_used_in_recursion, moved). Every one carries a reason comment.
No `#![allow]` inner attribute added anywhere -> the 6 arc_with_non_send_sync are all per-fn/per-stmt, none crate-wide.
Counts: too_many_arguments 16, dead_code 8, arc 6, assertions_on_constants 2, ptr_arg/unused_imports/await_holding_lock/only_used_in_recursion 1 each.
dead_code adjudication:
- JUSTIFIED (used under cfg(test)): seed_ref, seed_ref_with_editor, assert_ref_seeded (subsystem_seed.rs), await_main_pin_intent (frontend_slice/components.rs:1639), ref_non_seed_ids (used at subsystem_seed.rs:156 by assert_ref_seeded).
- JUSTIFIED (RAII anchor): popup_row_click_windowed.rs `services: Arc<TestServices>`.
- DEFECT D1: crates/holon-orgmode/tests/incremental_org_writeback_smoke.rs:90 `fn point_read_calls()` — zero call sites; the allow masks dead code CLAUDE.md says to DELETE.
- DEFECT D2: frontends/gpui/tests/chrome_one_row_windowed.rs:68 `const CHROME_ELEMENT_IDS` — zero references outside its own definition. Same.
D3 (report accuracy, not code): 2 of 16 too_many_arguments are PRODUCTION, not GPUI/harness —
crates/holon-org-format/src/parser.rs `process_headlines`, crates/holon-orgmode/src/di.rs `run_file_sync_controller`.

## C3 CONFIRMED
cargo clippy --workspace --features CANON --all-targets -- -D warnings -> CLIPPY_EXIT=0 (clippy.log).
just check-frontend-wasm -> Finished, 0 errors (wasm-frontend.log). just check-worker-wasm -> WORKER_EXIT=0, 0 errors.
cargo check -p X --all-features --all-targets: holon-filesystem 0, holon-logseq-db 0, holon-loro 0, holon-loro-wiring 0.
holon-loro-testing EXIT=101 — NOT the lane: 7x `holon_loro::multi_peer ... configured out`; multi_peer is
`#[cfg(any(test, feature="test-helpers"))]` (holon-loro/src/lib.rs:82). Adding `--features holon-loro/test-helpers`
-> PROBE_EXIT=0. Pre-existing feature-unification breakage; lane touched no [features] table or source there.
machete: "cargo-machete didn't find any unused dependencies in this directory. Good job!" -> serde_json ignore in holon-loro-testing confirmed.

## C4 CONFIRMED
cargo nextest run -p holon-api -> "Summary [5.742s] 519 tests run: 519 passed, 0 skipped", exit 0.
Boxing is schema-time only: FieldLifetime::Computed constructed at 8 sites (type_registry, tests), read via
`&**spec` / matches!; no hot path. Wire unchanged — my own standalone serde proof (scratchpad/verify-clippy/serdebox):
plain={"computed":{"spec":"x"}} boxed={"computed":{"spec":"x"}} WIRE_IDENTICAL=true, and plain JSON
deserializes into the boxed variant. FieldLifetime is Deserialize-d from type YAMLs (default_lifetime/lifetime).

## C5 CONFIRMED — and the decisive answer is YES
Stages run individually with the recipe's own commands (justfile:807-833; CANON justfile:26-27):
fmt `cargo fmt --all --check` FMT_EXIT=0, 0 lines. clippy CLIPPY_EXIT=0. machete "didn't find any unused dependencies".
jscpd JSCPD_EXIT=0 "No duplicates found." deny DENY_LANE_EXIT=4, exactly ONE error:
`error[rejected]: failed to satisfy license requirements` finl_unicode 1.4.0 `(MIT OR Apache-2.0) AND Unicode-DFS-2016`
<- cooklang 0.18.7; tail = "advisories ok, bans ok, licenses FAILED, sources ok".
DECISIVE: integration tip = 305a3d6b1b102eb120f7a75a9c6b8fe1727708f1; `git -C <primary> archive` -> 85 entries (non-empty).
cooklang and finl_unicode both count 0 in the tip Cargo.lock. Lane diff applied there (`git apply`, Cargo.lock hunks
applied -> 0.1.92; only crates/holon-kitchen/tests/{block_params,cook_ingest}.rs are absent, deleted by lowcode-inc3).
`cargo deny check` at tip+diff -> DENY_TIP_EXIT=0, "advisories ok, bans ok, licenses ok, sources ok".
=> `just lint` is fully green at the wave-11 tip with this diff. Lint CAN enter the landing gate after lowcode-inc3 lands.

## C6 CONFIRMED
23 removed `use` lines in the diff; `cargo clippy --workspace --features CANON --all-targets -- -D warnings` exits 0,
and --all-targets compiles lib/bins/tests/benches/examples incl. inline #[cfg(test)] modules — so no import removed
here is needed under test cfg. The pbt_harness re-export survives, pinned by `#[allow(unused_imports)]` at
frontends/gpui/tests/pbt_harness/mod.rs. No --fix casualty left.

## GAPS
G1. jscpd exits 0 having analyzed **0 files / 0 lines / 0 tokens** — a vacuous green stage; gating it buys nothing.
G2. Pre-existing: `cargo check -p holon-loro-testing --all-features` is red without holon-loro/test-helpers (see C3).
G3. I ran the five lint stages separately, not `just lint` as one recipe (commands taken verbatim from justfile:807-833).

## Rev 2 — CONFIRMED
pwd for every check: /Users/martin/Workspaces/pkm/holon/.claude/worktrees/clippy-reds; tree assert OK (async-trait 0.1.92).

D1 FIXED. `grep -rn "fn point_read_calls" crates frontends` -> 0 hits; the allow is gone with it.
The counter FIELD survives and is legitimately live: crates/holon-orgmode/tests/incremental_org_writeback_smoke.rs
:52 decl, :62 init, :113 `self.point_read_calls.fetch_add(1, Ordering::SeqCst)` — a field read, so dead_code
cannot fire. No replacement allow was added (see the allow-set delta below).

D2 FIXED. `grep -rn "CHROME_ELEMENT_IDS" crates frontends` -> 0 hits; const and allow both deleted.

D3 FIXED. Both production sites carry their reason comments in the tree:
- crates/holon-org-format/src/parser.rs:778 `#[allow(clippy::too_many_arguments)]` above `fn process_headlines`,
  preceded by "Nine arguments because six are threaded unchanged through the recursion; folding them into a
  context struct is a parser refactor, not a lint fix."
- crates/holon-orgmode/src/di.rs:1045 above `pub async fn run_file_sync_controller`, preceded by
  "Eight arguments because this is the bootstrap seam both factories call with independently-built
  collaborators; grouping them is a wiring refactor."

ALLOW COUNT 36 -> 34, and the drop is exactly the two defects, nothing else.
I diffed the allow SETS, not just the counts (sorted `^+.*#[allow` lines, rev1 vs rev2):
- `comm -13` (present in rev2 only) = EMPTY -> no NEW allow anywhere in the diff.
- `comm -23` (rev1 only) = exactly `#[allow(dead_code)]` x2 (one indented, one top-level) = D1 + D2.
Removed-allow count unchanged at 1 (the moved only_used_in_recursion).

LOCK UNCHANGED. `diff -q lock.diff lock2.diff` -> LOCK_UNCHANGED: still only the async-trait
0.1.91->0.1.92 version+checksum pair and the 7 machete dep removals. Nothing else moved.

MY OWN CLIPPY RUN (semaphore-wrapped, unique log clippy-rev2-verify.log, guard asserting rev 2 is applied
by refusing if CHROME_ELEMENT_IDS still exists):
`cargo clippy --workspace --features holon-integration-tests/pbt,holon-integration-tests/web-arm,holon-gpui/pbt
--all-targets -- -D warnings` -> **CLIPPY_REV2_EXIT=0**, "Finished `dev` profile ... in 4.39s".
Fully cached, i.e. cargo fingerprints match the current rev-2 file contents; a changed file would have
invalidated and rebuilt, so the exit 0 applies to the tree as it stands.

LANE GATE LOG corroborates (lane-logs/rev2-gate-8356.log, 8590 bytes): FMT_EXIT=0, CLIPPY_EXIT=0,
CHECK_EXIT=0, and no `cargo fix` / `clippy --fix` invocation appears in it.

G1 (jscpd analyses 0 files) and G2 (pre-existing holon-loro-testing --all-features red) remain open as recorded.
No new defect found in rev 2.
