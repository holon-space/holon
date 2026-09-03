# Adversarial verification — lane `lowcode-inc2a`

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/lowcode-inc2a` (jj workspace,
`@` = 9a4b336a on `main 4e2ee368`, uncommitted). Every command run there; no jj/git writes.

## Verdict: CONFIRMED

Every load-bearing claim was reproduced from scratch in this session. Two
observations and one hazard are recorded below; none refutes the claim.

## 1. Tree identity

`pwd` = the workspace. `crates/holon-plugin-host/Cargo.toml` and `guests/cooklang/`
both present. `jj status`: 29 paths, `crates/holon-plugin-host/plugins/cooklang.wasm`
tracked `A` at **516,971 bytes (505 KiB)** and `tests/fixtures/testkit.wasm` `A` at
79,032 bytes — both under the 1 MiB line, so NOT a finding.

## 2. Gates (reproduced, not trusted)

| gate | log | result |
|---|---|---|
| `nextest -p holon-plugin-host -p holon-rows -p holon-kitchen -p holon-core -p holon-architecture-tests` | `lane-logs/verify-nextest-103445.log` | **263 passed, 2 skipped** |
| `cargo check -p holon-gpui` | `lane-logs/verify-checks-*.log` | clean |
| `cargo check -p holon-frontend -p holon-plugin-host --target wasm32-unknown-unknown` | same | clean |
| `cargo fmt --check` | same | clean |
| `bugfunnel.py check` | same | `636 entries, 0 problems` |
| final re-run after all mutations were reverted | `lane-logs/verify-final-*.log` | **263 passed** |

Toolchain in-lane: `nightly-2026-08-16-aarch64-apple-darwin` (from the lane's own
`rust-toolchain.toml`), not the Homebrew stable.

## 3. Teeth — the tests actually bite

**(a) Guest drops a column.** Removed `"unit"` from the `ingredient_use` row in
`guests/cooklang/src/lib.rs`, rebuilt with `guests/build.sh cooklang`
(sha `a473b6d0…`). `lane-logs/verify-teeth-a-103716.log`: **3 of 4 differential
tests RED**, each naming the divergence explicitly —
`UNFILED DIVERGENCE — triage it with the bug-gap-triage skill …` followed by
`rows differ:` with both sides printed. No obscure panic. Restored.

**(b) Fuel limit removed.** Patched both `set_fuel` call sites in `src/host.rs`
to `u64::MAX`. `lane-logs/verify-teeth-b-*.log`: the `spin` guest **never
returns** — `timeout 90` killed it, exit 124. The fuel limit is what stops the
loop, not luck. Restored.

**(c) Anti-vacuity guard.** Made the generator emit an unclosed component brace
so BOTH legs refuse. `lane-logs/verify-teeth-c-*.log`: the PBT **FAILS**, it
does not pass — `the generator produced a recipe the native adapter refuses, so
this case proves nothing`, and `every_generated_recipe_…` fails with `the vault
generator must produce recipes the native adapter accepts`. Restored.

**Restore is byte-exact.** Rebuilding the guest from the restored source
reproduces the tracked artefact bit for bit:
`d3a1bb16c6e7d0dc1c39bd9e685568ee91d6151c7d4ff790dddf9c1659423083` == baseline.

## 4. Sidecar / guest admission (scratch tests, since deleted)

Each refuses loudly with a typed, causal error:

- missing `.wasm` → `plugin sidecar …/ghost.yaml is not admissible: format "ghost" names guest …/nope.wasm, which is not a file`
- non-wasm bytes → `guest …/junk.wasm of format "junk" does not load: wasm error: magic header not detected`
- **missing export** (hand-assembled 25-byte module exporting only `memory`) → `guest is missing required export `holon_alloc``
- malformed JSON Lines → `… emitted a stream for notes/case.testkit that is not the row contract: line 1 is not a row-stream envelope`
- undeclared scope → `emitted scope "mystery", which its sidecar does not declare`
- undeclared column → `emitted column "surprise" on a "thing" row, which its sidecar does not declare`

Repeated-use probes (the kept-alive instance is the risky seam): **200 traps**
then a good parse → still `Ok`; **50 fuel exhaustions** then a good parse →
still `Ok`. No poisoning, no observable leak at that scale.

## 5. "Faithful port" and the German timer

Line-for-line comparison of `guests/cooklang/src/lib.rs` against
`crates/holon-kitchen/src/cook.rs` (+ `rows.rs`, `file_format.rs`) found **no
reachable behavioural difference**: same bare `cooklang::parse`, same two brace
refusals byte for byte, same quantity/fraction/range/text handling, same
metadata flattening and title skip, same step/prose split, same block id
(`{file}::b::{seq}` on both), same slug + occurrence + `step_index`, same NULL
columns, and `servings` is omitted on BOTH sides (`rows.rs:34-37` = `lib.rs:107-117`
= `cooklang.yaml:15`).

German timer, verified in the crate source at
`~/.cargo/registry/src/*/cooklang-0.18.7/`:
`cooklang::parse` = `CooklangParser::default()` (`lib.rs:272`), whose
`extensions: Extensions` derives `Default` = **`Extensions::all()`**
(`lib.rs:153-158`), which includes **`ADVANCED_UNITS`** (`lib.rs:123`). That flag
guards the arm at `analysis/event_consumer.rs:977,992` which raises
`Unknown timer unit: {unit_text}` as an **error**. So `~{9%Minuten}` is refused
by both legs, for the same reason — the flag is `ADVANCED_UNITS`. The lane's
`a_german_timer_unit_is_refused_by_both_legs` passed in all 5 of my runs.

## 6. 200-recipe timing, reproduced once

`lane-logs/verify-latency-*.log`, release, one warmed `PluginHost`:

| | total | per recipe |
|---|---|---|
| plugin (wasmi) | **251.4 ms** | 1.257 ms |
| native `cook.rs` | **12.5 ms** | 0.062 ms |
| ratio | | **20.1x** |

Fuel/memory reproduce EXACTLY: `150679646 fuel, 2228224 bytes`. The **ratio**
(20.1x vs the lane's 21.6x) holds; both absolute figures are ~2x faster than the
lane's 517.5 / 23.9 ms — a machine-load difference, not a discrepancy in the
claim. The qualitative conclusion survives: a cold full 200-recipe scan
(251 ms) still exceeds the 200 ms SLO, per-interaction (1.3 ms) does not.

## 7. Observations (not refutations)

- **`PROPTEST_CASES` is inert for the differential PBT.** `ProptestConfig::with_cases(96)`
  (`cook_plugin_differential_pbt.rs:139`) is `Config { cases, ..Config::default() }`
  (proptest-1.9.0 `config.rs:456`), so it OVERRIDES the env-derived default. The
  requested `PROPTEST_CASES=256` cold run therefore ran 96 cases. I compensated by
  running the suite **5 times** (~480 distinct cases) plus the 40-recipe
  deterministic vault each time — all green (`lane-logs/verify-soak-*.log`).
- **Trap path skips `free`.** `PluginHost::parse` returns via `?` on a trap before
  the two `self.free(...)` calls (`src/host.rs`), so each trapped call leaks its
  input+ctx buffers in the live guest. 200 traps of 20 KB did not bite (64 MiB
  ceiling); thousands of traps on one long-lived host eventually would. Cosmetic
  today, worth a line in Inc 3.
- **Block local ids are not run through `checked_local_id`.** Typed rows are
  (`adapter.rs`), block ids are only prefixed with the file id. Bounded by the
  prefix, and the format is read-only, so nothing lands wrong.

## 8. One hazard for the orchestrator

**Nothing ties the tracked `cooklang.wasm` to `guests/cooklang/src/lib.rs`.** The
guest is an excluded workspace with its OWN `Cargo.lock` (`=0.18.7`) while
`holon-kitchen` takes `cooklang = "0.18"` from the main lock (also 0.18.7 today).
Two consequences: (1) a source edit without `guests/build.sh` leaves the whole
suite green on a stale binary; (2) a main-workspace `cargo update` silently moves
the native leg's parser while the guest stays pinned. The build IS byte-reproducible
(proven in §3), so a cheap gate is possible: rebuild in CI and assert the sha.
Recommend this before Inc 3 deletes `cook.rs`.

Tree restored: all mutated sources diff-clean against their backups, both `.wasm`
sha256s match baseline, scratch fixtures and the scratch test deleted, final gate
263/263 green.
