# archlint — Architecture Linter

*Part of [Architecture](../Architecture.md)*

## What it is

`archlint` is a fast, layered linter that enforces holon's architectural
rules. It lives at the repo root in `archlint/` and runs in three places:

1. **Per-edit hook** — wired into Claude Code's `PostToolUse` (matcher
   `Write|Edit|MultiEdit`). Every file write triggers `archlint hook`,
   which lints the changed file in ~150 ms and surfaces violations
   inline. See `.claude/settings.local.json`.
2. **CI / pre-commit** — `cargo test --workspace` picks up
   `crates/holon-architecture-tests/tests/architecture_rules.rs`, a
   51-line wrapper that shells out to `archlint --all` and panics with
   the diagnostic on failure. The full repo sweep takes ~2 s.
3. **On-demand** — `archlint check FILE`, `archlint --all`,
   `archlint dylint`, `archlint discover` for ad-hoc runs.

`archlint` replaces the previous ~1100-line cargo arch-test
implementation. The cargo path was too slow for a per-edit hook (some
tests took 30+ minutes) and couldn't drive the same rules from a
single source of truth.

## Three rule layers

```
┌──────────────────────────────────────────────────────────┐
│                  archlint check FILE                     │
│                  archlint hook                           │
│                  archlint --all                          │
└─────────────┬────────────────────────────────────────────┘
              │
   ┌──────────┴──────────┬────────────────────┐
   ▼                     ▼                    ▼
┌──────────┐     ┌──────────────┐    ┌──────────────────┐
│ ast-grep │     │  ripgrep     │    │  dylint cdylib   │
│   YAML   │     │  smell TOML  │    │  (LateLintPass)  │
│ ─────────│     │ ─────────────│    │ ─────────────────│
│ rules/   │     │ smells/      │    │ dylint/<name>/   │
│ ~5–50 ms │     │ <10 ms       │    │ ~1–2 s warm      │
│ per file │     │ per file     │    │ per crate; 70 s  │
│          │     │              │    │ cold cascade     │
│ CST      │     │ raw text     │    │ rustc HIR + type │
│ shapes   │     │ regex        │    │ resolution       │
└──────────┘     └──────────────┘    └──────────────────┘
```

| Need | Use |
|---|---|
| Match a textual pattern in source | smell (TOML) |
| Match a CST shape (no type info needed) | ast-grep YAML |
| Distinguish `Result.ok()` from `Option.ok()` | dylint |
| Resolve trait impls / call paths | dylint |
| Cross-function flow analysis | dylint |

Default to ast-grep when in doubt — it's an order of magnitude faster
to author and run. dylint is the right tool when correctness genuinely
depends on type information.

The first two layers run in `archlint --all` and the per-edit hook
(combined budget ~2 s for the full repo). The dylint layer runs only
via `archlint dylint` because it actually compiles target code with
`cargo check`; warm-cache cost is seconds-per-crate, cold cascade is
tens of seconds.

## File layout

```
archlint/
├── archlint                  # bash wrapper (picks Python ≥ 3.11)
├── archlint.py               # ~700-line runner
├── sgconfig.yml              # ast-grep config (ruleDirs: [rules])
├── rules/                    # ast-grep YAML rules — one file per rule
│   ├── ok.yml                                  # `.ok()` defensive pattern
│   ├── filter_map_ok.yml                       # `filter_map(|_| ... .ok())`
│   ├── unwrap_or_default.yml                   # `serde_json::from_*().unwrap_or_default()`
│   ├── no-block-on-in-async.yml                # block_on inside async fn
│   ├── no-jsonb-as-string.yml                  # `.as_string()` on jsonb cols
│   └── no-underscore-params.yml                # `fn f(_x: T)` masking unused
├── smells/                   # ripgrep regex smells (TOML)
│   ├── words.toml            # fallback / compatibility / global_registry / raw_sql
│   ├── imports.toml          # loro / turso / platform / frontend-provider-dep
│   └── focus.toml            # direct_focus_mutation / navigation_execute_op
├── dylint/                   # type-aware Rust lints (cdylib via dylint 5.0)
│   ├── README.md
│   ├── result_to_none/                       # `match r { Ok(x) => Some(x), Err(_) => None }`
│   ├── error_to_option_via_ok_question/      # `expr.ok()?` in Option-returning fn
│   ├── unit_error_type/                      # `Result<T, ()>`
│   ├── hardcoded_table_name/                 # matview names as bare literals
│   └── serde_skip_default_on_deserialize/    # silently-defaulted Deserialize fields
└── cache/
    └── jsonb-fields.json     # built by `archlint discover`; auto-refreshed
                              # when crates/**/*.rs is newer than cache mtime
```

## Suppression — the ALLOW protocol

Every rule emits a diagnostic that names a canonical ALLOW tag in its
own message text (e.g. `// ALLOW(block_on): <reason>`). archlint
auto-discovers these tags by scanning each rule's message — there is
no side-table of aliases. This means the canonical name lives with the
rule definition, and renaming the rule id doesn't break existing
suppressions in the codebase.

A diagnostic is suppressed when:

- The rule id appears as `// ALLOW(<rule_id>)`, OR
- Any tag mentioned via `ALLOW(<tag>)` in the rule's message appears as
  `// ALLOW(<tag>)`,

on the same line, the line above the match start, or any line within
the matched span (multi-line ast-grep matches are common).

For dylint lints, the standard rustc attribute applies:
`#[allow(<lint_name>)]`.

## How to add a rule

### Add an ast-grep YAML rule

```sh
$EDITOR archlint/rules/<rule_id>.yml
```

Schema:

```yaml
id: my-rule
language: rust
severity: error
message: |
  What it catches and why. End with:
  To suppress: `// ALLOW(my_rule_tag): <reason>`
rule:
  pattern: $EXPR.something()
  # optional `inside:` / `not:` / `constraints:`
```

The runner reads `archlint/rules/*.yml` automatically. No registry
to update. The ALLOW tag is whatever you put in the message.

### Add a regex smell

```sh
$EDITOR archlint/smells/<group>.toml
```

```toml
[[smell]]
id = "my_smell"
pattern = '\bsuspicious_word\b'
files = "**/*.rs"
exclude = ["frontends/mcp/**"]
case_sensitive = false
message = """
What this catches. To suppress: // ALLOW(my_smell): <reason>
"""
```

### Add a dylint lint

```sh
cd archlint/dylint
cargo dylint new --isolate <lint_name>
```

Implement `LateLintPass` in `src/lib.rs`, write a UI fixture in
`ui/main.rs`, copy actual stderr to `ui/main.stderr` to bless. The
runner picks up new lints by directory presence — no registry update.

See `archlint/dylint/README.md` for the full conventions.

## Configuration knobs

| Flag / env | Effect |
|---|---|
| `archlint --format=json` | Stable JSON shape on stdout: `{version, violations, files_scanned, diagnostics: [{id, file, line, message}]}`. Always emitted, even at 0 violations. |
| `archlint dylint --no-deps` | Forwarded to `cargo dylint --no-deps`; skip workspace-mate packages when running on a single crate. |
| `archlint dylint --abort-on-failure` | Opt out of `--keep-going`; bail on first per-crate failure. |
| `CARGO_NET_GIT_FETCH_WITH_CLI=true` | Set automatically for `archlint dylint` to bypass macOS libgit2 SSL flakiness. |

## Performance budget

| Scenario | Wall time |
|---|---|
| Per-file hook (ast-grep + smells, single .rs) | ~150 ms |
| `archlint --all` (full repo, ~700 .rs files) | ~2 s |
| `archlint dylint`, warm, no source change (small crate) | ~1.8 s |
| `archlint dylint`, warm, after edit | ~1.5 s |
| `archlint dylint`, cold cascade (one-time) | ~70 s |
| `cargo test --workspace` (the wrapper) | ~2.1 s |

The 10-second `PostToolUse` hook timeout has ~7 s of headroom on the
fastest path.

## Production rule set (current)

| Rule id | Layer | Tag | Maps to |
|---|---|---|---|
| `no-underscore-params` | ast-grep | `unused_param` | unused-variable warning suppression hygiene |
| `no-block-on-in-async` | ast-grep | `block_on` | `tokio::Runtime::block_on` inside async deadlocks |
| `jsonb-as-string` | ast-grep | `jsonb_as_string` | `Value::as_string()` on `#[jsonb]` cols always returns None |
| `ok` | ast-grep | `ok` | `.ok()` on Result discards the error |
| `filter_map_ok` | ast-grep | `filter_map_ok` | `iter.filter_map(\|_\| ... .ok())` drops errors silently |
| `unwrap_or_default` | ast-grep | `unwrap_or_default` | `serde_json::from_*().unwrap_or_default()` masks corrupt data |
| `loro` / `turso` | smell | `loro` / `turso` | `holon-orgmode` must not import Loro/Turso directly |
| `platform` | smell | `platform` | `holon-frontend` must be platform-agnostic |
| `raw_sql_in_frontend` | smell | `sql` | Frontends must not contain raw SQL (mcp exempt) |
| `frontend-provider-dep` | smell | `frontend-provider-dep` | Frontend Cargo.toml must not depend on provider crates |
| `global_registry` | smell | `global_registry` | No `Arc<RwLock<HashMap<String, Entity<…>>>>` registries |
| `fallback` | smell | `fallback` | The word "fallback" usually means a hidden failure mode |
| `compatibility` | smell | `compatibility` | "Compatibility" usually means a backwards-compat shim to delete |
| `direct_focus_mutation` | smell | `direct_focus_mutation` | `ui_state.set_focus(...)` outside the canonical setter |
| `navigation_execute_op` | smell | `navigation_execute_op` | `execute_op("navigation", ...)` outside the provider |
| `no-handoff-md-at-repo-root` | hardcoded | — | `HANDOFF_*.md` at the repo root is forbidden (AC-7) |
| `catch_unwind_debug` | hardcoded | `catch_unwind_debug` | `catch_unwind` + `debug!()` swallowing panics |
| `no-scattered-match-as-str` | aggregate (--all only) | — | Same string set in `match s.as_str()` across 3+ files → use enum |
| `result_to_none` | dylint | `result_to_none` | `match r { Ok(x) => Some(x), Err(_) => None }` |
| `error_to_option_via_ok_question` | dylint | `error_to_option_via_ok_question` | `expr.ok()?` where expr: Result and fn returns Option |
| `unit_error_type` | dylint | `unit_error_type` | `Result<T, ()>` carries no error info |
| `hardcoded_table_name` | dylint | `hardcoded_table_name` | matview/junction names as bare string literals |
| `serde_skip_default_on_deserialize` | dylint | `serde_skip_default_on_deserialize` | `#[serde(skip*)]` on `derive(Deserialize)` field |

### Cell-architecture rules (added by the Cells plan)

These gates lock in the Cells architecture (see [Storage](Storage.md), [Sync](Sync.md), [Operations](Operations.md)). They land per phase and tighten as fields move to cells.

| Rule id | Phase | Tag | What it catches |
|---------|-------|-----|-----------------|
| `no-block-content-resolver` | Phase 1 | `no_block_content_resolver` | Re-introducing any `*ContentResolver` trait or struct for blocks |
| `no-deleted-symbols-resurface` | Phase 1+ | — | `BlockContentResolver` / `live_content` / `set_live_content` / `EditableTextProvider` / `_expected_*` / `with_content_resolver` reappearing in source |
| `cells-only-constructed-in-registry-layer` | Phase 1 | `cells_construct` | `Cell::new` and cell backing constructors callable only from cell-registry code + tests |
| `no-raw-mutable-for-cell-fields` | Phase 2 | `cell_field_mutable` | Source files outside cell-registry crates declaring `Mutable<T>` where `T` is an entity field type listed in any registered `cell_fields()` schema (deny-list grows per migrated field) |
| `cells-are-sole-block-writer` | Phase 2 | `block_write_via_cells` | Direct `INSERT INTO block` / `UPDATE block SET` outside `SqlBlockProjector` and the LoroSyncController startup-seed code |
| `no-inbound-loro-sync-runtime` | Phase 2 | `loro_inbound_runtime` | Re-introducing the runtime SQL→Loro inbound subscription path; only startup seeding is allowed |

`cell-field-mutable` and `cells-are-sole-block-writer` start as ast-grep CST rules with allow-lists; the deny-lists grow as more fields migrate. See `archlint/rules/no-block-content-resolver.yml` (and siblings) for the canonical rule definitions.

## Related

- [Principles](Principles.md) — the "Fail Loud, Never Fake" and
  "Parse, Don't Validate" philosophies that the lints encode.
- `crates/holon-architecture-tests/tests/architecture_rules.rs` —
  the 51-line cargo wrapper that gives `archlint` CI integration via
  `cargo test --workspace`.
- `archlint/dylint/README.md` — when to choose dylint vs ast-grep,
  how to add a new lint, measured per-lint cost.
- `devlog/2026-05-05-archlint-prototype.md` — the initial design
  notes and parity matrix vs the previous cargo arch-tests.
