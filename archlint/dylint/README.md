# archlint/dylint

Type-aware Rust lints — the third layer in archlint, complementing the
ast-grep YAML rules and the ripgrep TOML smells. Use this layer when a
check needs information only the Rust compiler has: actual types,
trait resolution, control-flow facts.

## When to add a dylint lint vs. an ast-grep rule

| Need | Use |
|---|---|
| Match a textual pattern in source | smell (TOML) |
| Match a CST shape (no type info) | ast-grep YAML |
| Distinguish `Result.ok()` from `Option.ok()` | dylint |
| Look at the *types* of args / return values | dylint |
| Trait-resolution / impl checks | dylint |
| Cross-function flow analysis | dylint |

Default to ast-grep when in doubt — it's an order of magnitude faster
to author and run. dylint is the right tool when the lint's correctness
genuinely depends on type information.

## Layout

```
archlint/dylint/
├── README.md                                (this file)
├── result_to_none/                          `match r { Ok(x) => Some(x), Err(_) => None }`
├── error_to_option_via_ok_question/         `expr.ok()?` in Option-returning fn
├── hardcoded_table_name/                    matview/junction names as bare literals
├── serde_skip_default_on_deserialize/       silently-defaulted Deserialize fields
└── unit_error_type/                         `Result<T, ()>`
```

Each lint is a standalone crate with `[workspace]` empty (a dylint
quirk) — they're not in a single Cargo workspace. Build them
individually or via `archlint dylint`. Standard structure inside each:

```
<lint>/
├── Cargo.toml       cdylib + dylint_linting + clippy_utils
├── README.md        per-lint docs (where present)
├── rust-toolchain   pinned nightly (matches dylint 5.0.0)
├── src/lib.rs       LateLintPass implementation
└── ui/              compiletest fixtures
    ├── main.rs      sample code that triggers / doesn't trigger
    └── main.stderr  expected diagnostic output (copy from actual
                     stderr after a run to bless)
```

`serde_skip_default_on_deserialize` keeps `ui/main.rs` trivial because
its full fixture would need serde as a dev-dep; functional verification
happens when the lint runs against the real workspace.

## Running

```sh
# All lints, against the whole workspace (slow first run; ~minutes)
archlint dylint

# Forward args to cargo check — useful while iterating:
archlint dylint -- -p holon-engine            # one crate
archlint dylint -- --workspace --tests        # include test code
archlint dylint --no-deps -- -p holon-engine  # skip workspace-mate packages
archlint dylint --abort-on-failure -- ...     # opt out of --keep-going
```

The runner always passes `--keep-going` to `cargo dylint` so a single
crate's per-check failure (e.g. an emitted lint error) doesn't kill the
rest of the run. Pass `--abort-on-failure` to bail on the first failure
instead.

The runner also splits cargo-dylint flags from cargo-check flags via the
standard `--` separator (the cargo-check args go *after* `--`, even when
combined with `--no-deps` / `--abort-on-failure` from us).

Why isn't it folded into `archlint --all`? dylint compiles and runs
`cargo check` against the actual workspace, which takes minutes the
first time and ~1.5 s warm. Keeping it behind a separate subcommand
preserves `--all`'s ~2 s budget for the per-edit hook.

## Per-run cost (measured, holon-engine target)

| Scenario                                         | Wall time |
|--------------------------------------------------|----------:|
| Cold cache (first run, full transitive compile)  |     ~70 s |
| Warm, no source change                           |    ~1.8 s |
| Warm, after edit in target crate                 |    ~1.5 s |
| Warm, after edit in **lint source** (cdylib relink) | ~2.5 s |

Per-edit feedback for type-aware patterns is better served by
rust-analyzer in the IDE — it shares the rustc cache and runs
continuously. Use `archlint dylint` for on-demand / pre-commit checks.

## Adding a new lint

```sh
cd archlint/dylint
cargo dylint new --isolate <lint_name>
```

That scaffolds the directory above with placeholder `Cargo.toml`,
`src/lib.rs`, and a UI test. Replace the description, fill in
`LateLintPass::check_*`, write the UI fixture, and run the tests:

```sh
cd <lint_name>
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo test --release ui
```

(`CARGO_NET_GIT_FETCH_WITH_CLI=true` works around libgit2 SSL issues
when fetching the pinned `clippy_utils` git rev — affects fresh
machines, not subsequent builds.)

## First lint: `result_to_none`

Codifies the global CLAUDE.md rule

> **DO NOT** returning `null` or `None` in case of an error.
> **DO** throw an exception / return an `Err` / `Failure` / ... instead.

with a type-aware check that ast-grep can't supply on its own. See
`result_to_none/README.md` for details.

## Known compatibility note

dylint 5.0.0 pins to nightly-2025-09-18 (rustc 1.92.0-nightly).
`blake3` (via `holon-api` → almost everything) pulls
`constant_time_eq@0.4.3`, which requires rustc ≥ 1.95. The error fires
at workspace **resolution** (before any per-crate compile starts), so
`--keep-going` doesn't override it. Today the runner is verified clean
on `holon-engine` (the one workspace crate that doesn't pull
`holon-api`); a full-workspace run waits on:

- A newer `dylint`/`dylint_linting` release pinned to rustc ≥ 1.95, or
- Pinning `constant_time_eq` to a `<= 0.4.2` line that supports rustc 1.92.

Track at <https://github.com/trailofbits/dylint/releases>; bump the
per-lint `rust-toolchain` channel when an upstream release crosses 1.95.
