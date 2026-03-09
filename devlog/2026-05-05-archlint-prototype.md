# archlint prototype — handoff (2026-05-05)

> **Update — afternoon session 2026-05-05.** Parity verified for every cargo
> arch-test that was ported. Defensive-pattern rules added (4 new ast-grep
> rules + multi-line catch_unwind heuristic). `--format=json` output, cache
> freshness check, and ALLOW-tag auto-discovery from rule messages all landed.
> Per-rule details below. Open issues §1, §2, §3, §4, §5 are now closed; §7
> (cargo arch-tests' fate) is the only remaining decision.

A standalone, fast architecture-rule linter wired into the Claude Code
PostToolUse hook so the agent gets in-loop feedback on every Write/Edit/MultiEdit.

Replacement target: the cargo arch-tests at
`crates/holon-architecture-tests/tests/architecture_rules.rs` and
`crates/holon-architecture-tests/tests/no_defensive_programming.rs`. Those run
as `#[test]` against the whole repo via `cargo test`, are slow (5+ min for
some tests), and can't drive a per-edit hook. archlint runs in ~140 ms on a
single file and ~2 s for the full repo — fast enough for both the edit hook
and CI.

## Where it lives

```
archlint/
├── archlint                # bash wrapper (picks Python ≥ 3.11)
├── archlint.py             # ~700-line runner
├── sgconfig.yml            # ast-grep config (ruleDirs: [rules])
├── rules/                  # ast-grep YAML, one file per rule
│   ├── no-block-on-in-async.yml
│   ├── no-underscore-params.yml
│   ├── no-jsonb-as-string.yml
│   ├── ok.yml              # NEW — Result.ok() defensive-pattern
│   ├── filter_map_ok.yml   # NEW — filter_map(|_| ... .ok())
│   └── unwrap_or_default.yml  # NEW — serde_json::from_*().unwrap_or_default()
├── smells/                 # ripgrep regex smells (TOML)
│   ├── words.toml          # fallback / compatibility / global_registry / raw_sql_in_frontend
│   ├── imports.toml        # loro / turso / platform / frontend-provider-dep
│   └── focus.toml          # NEW — direct_focus_mutation / navigation_execute_op
├── dylint/                 # NEW — type-aware Rust lints (cdylib via dylint 5.0)
│   ├── README.md           # how to add a lint, when to prefer dylint over ast-grep
│   ├── result_to_none/     # `Ok(_) => Some(_), Err(_) => None` on Result
│   ├── error_to_option_via_ok_question/  # `expr.ok()?` in Option-returning fn
│   ├── hardcoded_table_name/             # matview names as bare literals
│   ├── serde_skip_default_on_deserialize/ # silently-defaulted Deserialize fields
│   └── unit_error_type/                  # `Result<T, ()>` carries no error info
└── cache/
    └── jsonb-fields.json   # built by `archlint discover`, read by hook
```

## How it runs

| Mode | Command | Use |
|---|---|---|
| Per-file | `archlint check FILE [FILE ...]` | direct invocation; exits 2 on violations |
| Edit hook | `archlint hook` | reads Claude Code JSON from stdin (`tool_input.file_path` or `edits[]`) |
| CI / full sweep | `archlint --all` | scans whole repo; runs aggregates; replaces `cargo arch-test` |
| Discovery | `archlint discover` | rebuilds `cache/jsonb-fields.json` |
| JSON output | `archlint --format=json --all` | machine-readable diagnostics on **stdout** for ralph/ouroboros pipelines; emits a stable `{version, violations, files_scanned, diagnostics:[…]}` shape even at 0 violations |
| Type-aware Rust lints | `archlint dylint [-- <cargo-check-args>]` | runs every lint under `archlint/dylint/`; slow first run (cargo check on the workspace + cdylib compile); not folded into `--all` to keep the per-edit budget tight |

The runner is now three layers:

1. **ast-grep YAML** (`rules/`) for CST-shape checks. Engine: tree-sitter,
   ~5–50 ms per file.
2. **ripgrep regex smells** (`smells/*.toml`) for raw-text checks (forbidden
   words, forbidden imports, raw SQL). Each smell:
3. **dylint cdylib lints** (`dylint/<name>/`) for type-aware Rust checks.
   Engine: rustc late lint pass, minutes-scale (it actually compiles the
   target). Behind a separate subcommand (`archlint dylint`); not in the
   per-edit hook path.

```toml
[[smell]]
id = "..."           # also serves as the ALLOW(<id>) suppress tag
pattern = "..."      # PCRE2 regex
files = "..."        # glob (default "**/*.rs")
exclude = ["..."]    # list of globs (or single string)
case_sensitive = false
message = "..."      # multi-line OK
```

Suppression is uniform across both layers: `// ALLOW(<id>): <reason>` on the
same or preceding line.

## Hook wiring (already applied)

`.claude/settings.local.json` PostToolUse list now contains, between the
existing `cargo fmt --all` entry and the `engram intercept` catchall:

```jsonc
{
  "matcher": "Write|Edit|MultiEdit",
  "hooks": [{
    "type": "command",
    "command": "/Users/martin/Workspaces/pkm/holon/archlint/archlint hook",
    "timeout": 10
  }]
}
```

Backup at `.claude/settings.local.json.archlint-backup-1777940202`.

To activate in an existing session, restart Claude Code. New sessions pick it
up automatically.

## Rule status

| Original test | Status | Notes |
|---|---|---|
| `orgmode_no_direct_loro_or_turso_imports` | ✅ ported | smells/imports.toml — split into `loro` + `turso` smells (matches original ALLOW tags) |
| `frontend_cargo_no_provider_deps` | ✅ ported | smells/imports.toml — `frontend-provider-dep` |
| `no_raw_sql_in_frontends` | ✅ ported | smells/words.toml — `raw_sql_in_frontend` |
| `frontend_crate_no_platform_imports` | ✅ ported | smells/imports.toml — `platform` |
| `no_underscore_prefixed_params` | ✅ ported | rules/no-underscore-params.yml (ast-grep) |
| `no_block_on_in_async_context` | ✅ ported | rules/no-block-on-in-async.yml (ast-grep + `inside` ancestor check) |
| `no_global_entity_registries` | ✅ ported | smells/words.toml — `global_registry` |
| `no_handoff_md_at_repo_root` | ✅ ported | hardcoded check in `archlint.py::check_handoff_root` |
| `no_as_string_on_jsonb_columns` | ✅ ported | rules/no-jsonb-as-string.yml + discovery cache + post-filter on captured KEY |
| `no_scattered_string_matching` | ✅ ported | aggregate in `archlint.py::aggregate_scattered_match_as_str` (--all only) |
| `no_direct_focus_mutation` | ✅ ported (new) | smells/focus.toml — `direct_focus_mutation` |
| `no_navigation_execute_op_in_tests` | ✅ ported (new) | smells/focus.toml — `navigation_execute_op` |
| `no_result_dot_ok_in_production_code` | ✅ ported (new) | rules/ok.yml + post-filter for universal allow markers |
| `no_filter_map_ok_in_production_code` | ✅ ported (new) | rules/filter_map_ok.yml + must-contain-`.ok()` post-filter |
| `no_unwrap_or_default_on_deserialize` | ✅ ported (new) | rules/unwrap_or_default.yml |
| `no_catch_unwind_at_debug_level` | ✅ ported (new) | multi-line heuristic in `archlint.py::check_catch_unwind_at_debug` |

Plus two new rules not in the original cargo test:

- `fallback` (smell) — `\bfallback\b`
- `compatibility` (smell) — `\bcompatibilit(y|ies)\b`

These are user requests from the design conversation: backwards-compat shims
and silent-fallback patterns are smells per CLAUDE.md → Error Handling
Philosophy.

## Parity matrix (verified 2026-05-05 PM)

archlint --all hit counts vs `cargo test --test architecture_rules <NAME>` /
`cargo test --test no_defensive_programming <NAME>`:

| Rule | archlint | cargo | match? |
|---|---:|---:|:---:|
| `no_underscore_prefixed_params` | 327 | 327 | ✓ |
| `no_result_dot_ok_in_production_code` | 43 | 43 | ✓ |
| `no_filter_map_ok_in_production_code` | 4 | 4 | ✓ |
| `no_unwrap_or_default_on_deserialize` | 0 | 0 | ✓ |
| `no_catch_unwind_at_debug_level` | 0 | 0 | ✓ |
| `no_block_on_in_async_context` | 0 | 0 | ✓ |
| `no_raw_sql_in_frontends` | 4 | 4 | ✓ |
| `no_handoff_md_at_repo_root` | 1 | 1 | ✓ |
| `no_direct_focus_mutation` | 0 | 0 | ✓ |
| `no_navigation_execute_op_in_tests` | 0 | 0 | ✓ |
| `orgmode_no_direct_loro_or_turso_imports` | 0 | 0 | ✓ |
| `frontend_cargo_no_provider_deps` | 0 | 0 | ✓ |
| `frontend_crate_no_platform_imports` | 0 | 0 | ✓ |
| `no_global_entity_registries` | 0 | 0 | ✓ |
| `no_scattered_string_matching` | 0 | 0 | ✓ |
| `no_as_string_on_jsonb_columns` | 0 | 0 | ✓ |

The `no_underscore_prefixed_params` 327 is accumulated debt — both tools agree.
For all other rules, parity is exact. Three notable alignments worth recording:

1. **`no_block_on_in_async_context`**: archlint's ast-grep rule walks up to
   `async fn main()` in `frontends/ply/src/main.rs`, which the cargo test
   skips by file path. Mirrored in archlint via `RULE_EXTRA_FILE_SKIPS`
   (`frontends/*/src/main.rs`, `lib.rs`, etc.).

2. **`no_raw_sql_in_frontends`**: archlint's smell pattern originally
   included `UPDATE` and `DELETE` keywords; cargo deliberately omits them
   (frontend mutations through `holon-worker/src/seed.rs` are accepted).
   Aligned to cargo's set: `SELECT|INSERT|CREATE TABLE|DROP|ALTER`.

3. **ALLOW tag mismatches** between rule id and the canonical short tag
   used in code (`block_on`, `jsonb_as_string`, `unused_param`, `sql`) are
   now resolved automatically: archlint extracts every `ALLOW(<tag>)`
   mentioned in a rule's own message text and accepts any of them as a
   valid suppression. The convention stays with the rule definition.

## Latency

| Scenario | Time |
|---|---|
| Single clean file | ~140 ms |
| Single dirty file (3 violations) | ~140 ms |
| Three files together | ~150 ms |
| Full repo (`--all`, ~700 Rust files + Cargo.tomls + .md) | ~2.0 s |

Per-file went from ~105 → ~140 ms after the jsonb-cache-freshness check
(stat-walk over `crates/**/*.rs`, ~30 ms on macOS). Most of the per-file time
is still Python startup (~50 ms). If that ever bites, port the runner to a
Rust binary or precompile bytecode. Hitting cold caches (no `target/`,
`/tmp/.archlint-cache` cleared) is still under the 10 s hook timeout by
~7 s of headroom.

## Current findings (from `archlint --all` after default-skip filter)

```
archlint: 525 architecture violation(s)
  [no-underscore-params]    327
  [fallback]                109
  [ok]                       43
  [compatibility]            37
  [raw_sql_in_frontend]       4
  [filter_map_ok]             4
  [no-handoff-md-at-repo-root] 1
```

All counts cross-checked against the canonical cargo arch-tests — see the
parity matrix above. The 327 `no-underscore-params` and 4 `raw_sql` hits are
accumulated debt that both tools agree on. The 109 `fallback` and 37
`compatibility` hits are new — added as smells per the user's CLAUDE.md
"Error Handling Philosophy", not previously caught.

The 1 HANDOFF hit is `HANDOFF_DATA_CDC_SCOPE_LEAK.md` — a real file at the
repo root (verified). The earlier devlog comment ("archlint --all confirmed
there is no HANDOFF_*.md at the repo root") was stale. Either relocate the
file to `holon-pkm/Projects/Holon/` per AC-7 or add `// ALLOW(...)` if it's
intentionally there for the duration of an investigation.

Worktree exclusion (`.claude/worktrees/agent-*/crates/...`) was independently
verified: `archlint --all` output contains 0 references to those paths
because `collect_all_files` only globs `crates/` and `frontends/` directly
under `REPO_ROOT`.

## Open issues / next session

1. ~~Complete parity matrix.~~ **DONE** — see Parity matrix table above. All
   16 ported rules verified. 327 underscore_param hits are shared debt
   between both tools.

2. ~~HANDOFF false positive.~~ **RESOLVED** — `HANDOFF_DATA_CDC_SCOPE_LEAK.md`
   actually exists at the repo root (the previous devlog comment was stale).
   The rule fires correctly. The fix is to relocate or `ALLOW`-suppress that
   file, not to change archlint.

3. ~~Worktree exclusion.~~ **VERIFIED** — `archlint --all` output contains
   0 references to `.claude/worktrees/`. `collect_all_files` only globs
   `crates/` and `frontends/` directly under `REPO_ROOT`, so parallel
   worktree copies don't leak in.

4. ~~Defensive-pattern rules.~~ **DONE** — 3 ast-grep rules
   (`rules/ok.yml`, `rules/filter_map_ok.yml`, `rules/unwrap_or_default.yml`)
   plus a multi-line catch_unwind+debug! heuristic in
   `archlint.py::check_catch_unwind_at_debug`. Post-filters mirror the
   universal allow logic from `no_defensive_programming.rs`.

5. ~~`--format=json` output.~~ **DONE** — top-level `--format={text,json}`
   flag. JSON shape is `{version, violations, files_scanned, diagnostics:
   [{id, file, line, message}]}` and is always emitted on stdout (even at 0
   violations) so downstream tooling sees a stable schema. Text output
   stays on stderr for the existing CLI experience.

6. ~~Discovery cache invalidation.~~ **DONE** — `jsonb_cache_is_fresh()`
   compares cache mtime against every file in the discovery scope and short-
   circuits on the first stale source. `cmd_check`/`cmd_hook` now call
   `get_jsonb_fields_with_refresh()` which rebuilds when stale.

7. ~~Decide cargo arch-tests' fate.~~ **DONE** (option 1, thin wrapper).
   - `crates/holon-architecture-tests/tests/architecture_rules.rs` reduced
     from ~720 lines to a 51-line `#[test] fn archlint_all_passes()` that
     spawns `archlint/archlint --all` and panics with the full diagnostic
     on non-zero exit.
   - `crates/holon-architecture-tests/tests/no_defensive_programming.rs`
     deleted (~250 lines).
   - `crates/holon-architecture-tests/Cargo.toml` dev-dependencies cleared
     (no `ast-grep-core`, `ast-grep-language`, `glob` needed; archlint owns
     all that).
   - CI (`cargo test --workspace`) keeps working unchanged. archlint is
     the single source of truth.
   - Wrapper runs in 2.13 s; previously the slowest arch-test
     (`no_as_string_on_jsonb_columns`) took 33 min on the same scope.

## dylint layer — type-aware Rust lints (afternoon)

Per the user's request: "have a `dylint` in `archlint` so we extend something
existing instead of starting from scratch when we need it." The scaffolding
lives at `archlint/dylint/`. Five lints to start, each mapping to a CLAUDE.md
rule or a past MEMORY.md regression:

- **`result_to_none`** — `match r { Ok(x) => Some(x), Err(_) => None }`. Type
  info needed to confirm scrutinee is `Result<_, _>`. CLAUDE.md global
  *"DO NOT returning null or None in case of an error"*.
- **`error_to_option_via_ok_question`** — `expr.ok()?` where `expr: Result`
  and the enclosing fn returns `Option`. Same rule, different shape; pairs
  with `result_to_none` to cover both common forms.
- **`unit_error_type`** — `Result<T, ()>` (return type or binding annotation).
  Project CLAUDE.md *"NEVER swallow errors!! Use Result and enrich the
  error message with information"* — `()` is the empty error.
- **`hardcoded_table_name`** — string literals matching the matview/junction
  set (`block`, `block_raw`, `block_tags`, `task_blockers`, `focus_roots`)
  must come from the typed constants in
  `crates/holon/src/storage/block_table_names.rs`. Direct hit on the May
  2026 LoroSyncController regression.
- **`serde_skip_default_on_deserialize`** — fields with `#[serde(skip)]` /
  `#[serde(skip_deserializing)]` on a struct that derives `Deserialize`
  silently fill with `Default::default()`. Direct hit on the May 2026
  "Block has two deserializers" regression (`tags = []` on every SQL row).

All five build clean and run via `archlint dylint`. Compile budget:
~1 s per lint warm-cache, 8–35 s cold per lint (first-time clippy_utils
compile dominates). UI fixtures verify positive/negative cases for the
first four; the serde lint is verified by running against real workspace
code (its UI fixture would need serde as a dev-dep).

How it slots in:

- Layer 3 of the runner. Not in `archlint --all` (cargo check on the
  workspace is multi-minute first run; would blow the per-edit budget).
- New subcommand: `archlint dylint [-- <cargo-check-args>]`. Each lib under
  `archlint/dylint/<name>/` is loaded and run via `cargo dylint --path`.
- Adding new lints: `cd archlint/dylint && cargo dylint new --isolate <name>`,
  fill in the `LateLintPass`, write a UI fixture. No registry to update.

Compatibility note: dylint 5.0.0 pins to nightly-2025-09-18 (rustc
1.92.0-nightly). A few crates in the holon workspace pull in
`constant_time_eq@0.4.3` (via `blake3` via `holon-api`) which requires
rustc ≥ 1.95. The error fires at workspace dep-resolution, before any
crate gets checked, so `--keep-going` cannot rescue it. `archlint
dylint` is verified clean on `-p holon-engine` (the one workspace
member that doesn't pull `holon-api`); a full-workspace run waits on a
dylint upgrade to a rustc-1.95+ nightly or a workspace dep pin.

Optimizations applied: the runner passes `--keep-going` by default
(opt-out via `--abort-on-failure`) so per-crate check failures don't
kill the multi-lint run, and exposes `--no-deps` to skip workspace
mate packages when targeting a single crate. Measured warm-cache cost
on `holon-engine`: ~1.5 s after a target-crate edit, ~2.5 s after a
lint-source edit (cdylib relink), ~70 s cold.

## Auto-discovery of ALLOW tags (afternoon refactor)

Previously archlint had a hardcoded `RULE_ALLOW_ALIASES` dict mapping ast-
grep rule ids (e.g. `no-block-on-in-async`) to the canonical short ALLOW
tag (`block_on`). This forced the canonical tag name to live in two places.

Now: the rule's own message text is the source of truth. Each YAML rule's
`message:` already says `To suppress: \`// ALLOW(<tag>): <reason>\``;
archlint scans every diagnostic's `message` for `ALLOW(<tag>)` patterns and
accepts every one of them as a valid suppression marker (in addition to
the rule id). The dict is gone. Side-effect: a previously-broken alias for
`raw_sql_in_frontend` (rule id) ↔ `sql` (ALLOW tag in code) now works.

## How to pick this up tomorrow

1. Sanity check: `archlint/archlint check crates/holon/src/api/backend_engine.rs`
   — should print 0 violations and exit 0.
2. Full sweep: `archlint/archlint --all 2>&1 | tee /tmp/archlint.out`
   — should match the numbers above (±1 if files have changed).
3. Pick up at "Open issues" §1 or §4 depending on priority.

## Files changed this session

Morning session (initial prototype):
- `archlint/` (new directory + 8 files)
- `.claude/settings.local.json` (added one PostToolUse hook entry)
- `devlog/2026-05-05-archlint-prototype.md` (this file)
- Backup: `.claude/settings.local.json.archlint-backup-1777940202`

Afternoon session (parity + completion):
- `archlint/rules/ok.yml` — new
- `archlint/rules/filter_map_ok.yml` — new
- `archlint/rules/unwrap_or_default.yml` — new
- `archlint/smells/focus.toml` — new (direct_focus_mutation, navigation_execute_op)
- `archlint/smells/words.toml` — narrowed `raw_sql_in_frontend` pattern to match cargo
- `archlint/archlint.py` — ALLOW-tag auto-discovery, defensive post-filters,
  catch_unwind heuristic, `--format=json`, jsonb-cache freshness check,
  `RULE_EXTRA_FILE_SKIPS`, full-match-range `has_allow`
- `crates/holon-architecture-tests/tests/architecture_rules.rs` — replaced
  with 51-line thin wrapper that shells out to archlint
- `crates/holon-architecture-tests/tests/no_defensive_programming.rs` — deleted
- `crates/holon-architecture-tests/Cargo.toml` — dev-deps cleared
