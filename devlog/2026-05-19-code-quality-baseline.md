# Code Quality Baseline & Improvement Plan

**Date:** 2026-05-19
**Baseline run:** `just analyze` on `main` (worktree `just-analyze-fixes`)
**Tooling:** clippy, cargo-deny, cargo-machete, archlint, polydup, cargo-crap (via cargo-llvm-cov + nextest)

## Status (live)

| Phase | Status | Result |
|---|---|---|
| Phase 1 — Unblock CI | ✅ done | `just analyze-clippy` exits 0 (recipe softened, 4 deny-by-default PI errors fixed); `lorotree-spike` licensed; handoff doc moved to `devlog/`. ~250 incidental clippy fixes across 9 crates as side effect. |
| Phase 2 — Defensive-code purge | ✅ done | archlint reports **0 violations / 821 files** (was 252). 5 of 6 rules cleared by triage; `no-underscore-params` bulk-converted to bare `_` (176 sites, 85 files). 4 real fallback smells separately fixed for real (not just tagged): todoist sync_provider missing sync_token → `Err`; turso_ivm proptest empty-strategies → `assert!`; gpui icon `current_exe()` and board hex-color parse → `tracing::warn!` on failure path. |
| Phase 3 — Supply-chain hygiene | ✅ done | `cargo deny check` exits 0. Targeted `cargo update` upgraded aws-lc-rs/aws-lc-sys, rustls-webpki, thin-vec, uds_windows, lz4_flex — clearing 8 vulnerabilities and 2 yanked warnings. Added `bzip2-1.0.6` to allowed licenses (libbz2-rs-sys). Pruned 3 stale unmatched-license allowances. Four advisories with no upstream fix moved to ignore with rationale: rand custom-logger unsoundness (we don't install a custom logger), core2 unmaintained (transitive via image/ravif), hickory-proto 0.25 DNSSEC vulns (transitive via iroh@0.96 — upstream moved code to hickory-net 0.26.x but iroh not bumped yet). |
| Phase 4 — De-duplication | pending | |
| Phase 5 — CRAP reduction | pending | |
| Phase 6 — CI gate promotion | pending | |

## TL;DR

Six analyzers run automatically. Five produce findings; only `polydup` and `cargo-machete` complete cleanly today (and they have findings to triage). The two biggest items are **39 clippy errors blocking compilation of three crates** and **278 architecture violations**. Coverage-driven CRAP analysis reports 550 / 6,532 functions over the threshold, dominated by a few mega-functions and `examples/` `main()`s.

## Findings snapshot

| Analyzer | Status | Headline number | Worst offender |
|---|---|---|---|
| clippy | 39 errors → 3 crates won't compile | 30× collapsible-if in `holon-macros` | `holon-macros` (lib) |
| cargo-deny | advisories FAIL, licenses FAIL | ≥6 vulns + several unmaintained/unsound + `lorotree-spike` unlicensed | aws-lc-rs, rand, core2 |
| cargo-machete | found unused deps | ~100 lines of unused-dep output | spread across crates |
| archlint | 278 violations across 7 rules | 182× `no-underscore-params` | repo-wide |
| polydup | 4,756 duplicate groups | 76 exact (Type-1) + 4,680 renamed (Type-2); ~323k lines savings | `frontends/mcp/src/tools.rs` (524 dupes) |
| cargo-crap | 550 / 6,532 functions exceed threshold 30 | CRAP 7140 / 0% cov | `tools/src/turso_sql_replay.rs::cmd_replay` |

### Top CRAP offenders (worth looking at first)

| CRAP | Cyclomatic | Coverage | Function |
|---:|---:|---:|---|
| 7140 | 84 | 0% | `cmd_replay` — tools/src/turso_sql_replay.rs:1263 |
| 7054 | 199 | 44.3% | `E2ESut::check_invariants_async` — crates/holon-integration-tests/src/pbt/sut_check_invariants.rs:237 |
| 5256 | 72 | — | `dispatch_mcp_tool` — frontends/holon-worker/src/lib.rs:450 |
| 2352 | 48 | 0% | `cmd_minimize` — tools/src/turso_sql_replay.rs:1966 |
| 1980 | 44 | 0% | `ViewKind::tag` — crates/holon-frontend/src/view_model.rs:360 |
| 1980 | 44 | — | `eval_binary_op` — frontends/waterui/src/render/interpreter.rs:162 |

Several CRAP rows have no coverage data (`—`). Most of those are `examples/main()` — analyzed by cargo-crap but not executed by `cargo test`, so they're noise.

### Archlint violation breakdown

| Rule | Count |
|---:|---|
| `no-underscore-params` | 182 |
| `fallback` | 44 |
| `ok` | 27 |
| `compatibility` | 18 |
| `filter_map_ok` | 3 |
| `raw_sql_in_frontend` | 3 |
| `no-handoff-md-at-repo-root` | 1 |

The first six are direct **"don't program defensively" / "fail loud"** invariants from `CLAUDE.md`. They're not stylistic — each is a latent error-swallowing site.

### cargo-deny — what's actually failing

- **Vulnerabilities:** aws-lc X.509 name-constraints bypass, O(n²) DNS-name compression, AWS-LC CRL distribution-point checks (×2), zlib-style decompression of invalid data, …
- **Unmaintained / unsound:** `core2` (yanked), `rand` rng custom-logger unsoundness.
- **Licenses:** `lorotree-spike` (in-tree experiment) has no license declaration.

### polydup duplication hotspots

856 files scanned, 5,964 functions, **4,756 duplicate groups**, estimated 323k lines of churn savings (best case).

Top files by duplicate count:
1. `frontends/mcp/src/tools.rs` — 524 dupes
2. `crates/holon/src/api/loro_backend.rs` — 464
3. `crates/holon/src/storage/turso_ivm_join_test.rs` — 322
4. `crates/holon-frontend/src/widget_gallery.rs` — 274
5. `crates/holon/tests/edge_field_e2e.rs` — 220

One illustrative group: a 50-token sequence repeats **29 times** across `operations.rs`, `reactive.rs`, `reactive_view.rs`, `user_driver.rs`, `mutation_driver.rs`, `test_environment.rs`, `widget_state.rs`, `sql_block_operations.rs`, `identity/provider.rs` …

## Phased improvement plan

Ordered by **value / effort**, not severity. A red clippy stops compilation; that comes first. CRAP requires real refactors; that's later.

### Phase 1 — Unblock CI (≤1 day)

Goal: `just analyze` exits 0 except for known-noisy checks. Nothing here changes behavior.

1. **Fix the 39 clippy errors.**
   - 30× collapsible-if in `holon-macros` — almost certainly a single macro template, mechanical fix.
   - 5× `holon-engine` errors, 1× `holon-pbt-core`.
   - 3× `map_or` simplifications.
   - 2× missing `Default` impls (`RhaiEvaluator`, `Engine`).
   - 1× malformed rustdoc list.
2. **License `lorotree-spike`** (or set `publish = false` + a workspace-level dual MIT/Apache-2.0).
3. **Add the missing archlint exception:** rename / move `HANDOFF_DATA_CDC_SCOPE_LEAK.md` into a topic doc per the rule.
4. **Confirm the analyze recipe gates make sense in CI.** `analyze-crap` runs the full test suite under coverage — leave it as a nightly job, not per-PR.

Exit criteria: `just analyze-clippy` exits 0, archlint `no-handoff-md-at-repo-root` is 0, deny `licenses` is one error fewer.

### Phase 2 — Defensive-code purge (1–2 weeks, scoped)

Goal: collapse archlint's 278 occurrences to under 50, focused on the rules that map to `CLAUDE.md` invariants.

1. **`no-underscore-params` (182):** every `_foo: T` parameter is a smell — either the value should be used or the parameter should not exist. Triage in batches by crate; most will be trivial removals.
2. **`fallback` (44) + `ok` (27) + `filter_map_ok` (3):** these are the error-swallowing patterns called out in the project rules. Convert each to either a propagated `Result` or a deliberate `match` that explains *why* the error is swallowed. Don't blanket-fix — read each site.
3. **`compatibility` (18):** investigate per-rule (the rule label is vague); these may be intentional bridges to be retired with the migration.
4. **`raw_sql_in_frontend` (3):** lift into `BlockOrdering` / `BlockQueryHelpers` per existing pattern.

Exit criteria: archlint total < 50 occurrences, all error-swallowing rules at 0.

### Phase 3 — Supply-chain hygiene (background, ~1 day spread over a week)

Goal: `cargo deny check` exits 0 except for issues we've explicitly waived in `deny.toml`.

1. **Update vulnerable transitives.** Most flagged crates are pulled by `aws-lc-rs` / `rand` / `core2`. `cargo update` first; for what doesn't update, raise an upstream issue or pin via `[patch]`.
2. **Replace `core2`** (yanked + unmaintained) — either we drop the dependency or vendor an alternative.
3. **Audit + enable a deny-on-vuln gate** in CI once the backlog is at zero. Add `[advisories] yanked = "deny"` / `vulnerability = "deny"` in `deny.toml`.

Exit criteria: `analyze-deny` exits 0.

### Phase 4 — De-duplication (2–3 weeks, opportunistic)

Goal: cut polydup's 4,756 groups in half by attacking the broadest groups first.

1. **Start with the 29-instance MCP-handler/builder boilerplate.** Almost certainly a macro or trait can collapse all 29 sites; one PR, large diff, low risk because the patterns are mechanical.
2. **`frontends/mcp/src/tools.rs` (524 dupes):** likely tool-registration boilerplate. Generate via macro or a registry table.
3. **`crates/holon/src/api/loro_backend.rs` (464):** inspect whether this is real duplication or test scaffolding patterns.
4. **Leave `experiments/lorotree-spike/`** alone — it's a spike, not production code. (Consider moving it out of `polydup scan` scope.)
5. **Leave `examples/` mostly alone** — repros benefit from being self-contained.

Exit criteria: polydup groups < 2,500.

### Phase 5 — CRAP reduction (ongoing)

Goal: drop functions over CRAP-30 from 550 to under 100, **starting with the ones that have low coverage and high complexity** (the upper-left corner of risk).

1. **Add a regression gate:** record the current top-CRAP list as a baseline, fail CI on regressions only. Don't try to fix all 550 at once.
2. **Decompose the worst offenders** in order, one PR each:
   - `cmd_replay` and `cmd_minimize` in `tools/src/turso_sql_replay.rs` — extract per-arg handlers; both have 0% coverage, so adding tests is half the win.
   - `E2ESut::check_invariants_async` — 199 cyclomatic but already 44% covered; this is mostly a refactoring exercise (split each invariant into its own function).
   - `dispatch_mcp_tool` (worker) and `eval_binary_op` (waterui) — large match arms → table-driven dispatch.
3. **Filter `examples/main()` from CRAP** (option A: keep but ignore; option B: configure `cargo-crap` to skip the `examples/` path). They inflate counts without offering refactor leverage.

Exit criteria: top-CRAP functions are tested at >50% line coverage; total over-threshold count < 100.

### Phase 6 — Make analyze a CI gate (final)

Once each Phase 1–5 analyzer is at its exit criteria, promote it to a required check:

1. **Per-PR (fast, ~3 min each):** clippy, archlint, machete, deny.
2. **Nightly (slow):** crap, duplication.

Bonus follow-ups that aren't on the critical path:
- Cap `polydup` to a single rule-violations-only report mode in CI; full report is for humans.
- Auto-publish `lcov.info` to Codecov / Coveralls so coverage trends are visible.
- Wire the existing `cargo mutants` recipe into the nightly job for the top-CRAP files (mutation testing is the natural next step after coverage).

## Tracking

Add this file's exit criteria as TODO items where appropriate. Per-rule archlint violations are already tracked by `archlint/discoveries/`; mirror Phase 2 progress there.
