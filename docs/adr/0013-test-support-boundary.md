# ADR 0013: Test-support boundary

**Status:** Accepted (2026-06-12; embodies spec 0007 Phase 3 item 4 — "decide, don't drift")
**Deciders:** Martin
**Context:** Phase 3 item 4 of spec 0007 calls for moving test support code behind a
feature gate or into a `holon-test-support` crate, and replacing ad-hoc `HOLON_PBT_*`
env switches in production paths. Spec offers the explicit alternative: "document 'test
seams are prod API' as a deliberate ADR — decide, don't drift."
**Relates to:** ADR 0007 (spec 0007 — Phase 3 crate hygiene), Phase 3.6 test relocation.

## Problem

Holon's test infrastructure lives in three places with three different statuses:

1. **Pure test modules** that happen to be compiled unconditionally in production
   crate builds: `pbt_infrastructure.rs` in `crates/holon/src/api/` (709 lines,
   proptest-based property-test infrastructure used only by `#[cfg(test)]` code and
   by integration-test binaries). It is gated only by `not(wasm32)` — production
   native builds pay compile time and expose the public API surface without ever
   calling it.

2. **Production types with test-adjacent behavior**: `ReactiveEngineDriver` and
   `HeadlessEditorMirror` in `crates/holon-frontend/src/`. These types are the
   engine-level `UserDriver` implementation — every frontend's MCP server
   (`click`/`type_text`/`describe_ui`) drives the UI through a `UserDriver`. The
   TUI frontend wraps `ReactiveEngineDriver` as its inner driver
   (`frontends/tui/src/user_driver.rs:43`). Gating them behind a test-only feature
   would break the TUI and the GPUI MCP tools' production contract.

3. **Env-var seams in production paths**: quiescence/drop-timeout overrides at
   `holon-frontend/src/user_driver.rs:945–957` (`HOLON_PBT_QUIESCENCE_MS`,
   `HOLON_PBT_DROP_TIMEOUT_MS`) and `HOLON_PBT_SUPERHUMAN_INPUT` at
   `frontends/gpui/src/user_driver.rs:905`. These are module-level free functions
   that read env vars at construction time — self-contained, default-off pacing
   knobs used by the PBT harness to tune driver timing. They never alter behavior
   for normal users.

4. **Harness configuration in the test crate**: ~14 `HOLON_PBT_*` vars live in
   `crates/holon-integration-tests/` — the PBT harness itself. These configure
   invariant modes, step budgets, weight overrides, capture naming — test harness
   configuration, not a boundary violation.

Phase 3.6 established the pattern for category 1: the `test-helpers` feature (empty,
zero-dependency, purely compile-time) in `crates/holon/Cargo.toml`, gating
`storage::test_helpers`, `di::test_helpers`, and `e2e_test_helpers` modules.

## Decision

**ADR-first; gate only `pbt_infrastructure`.**

Apply the Phase 3.6 pattern to the one remaining category-1 module
(`pbt_infrastructure.rs`), and document categories 2–4 as deliberate choices:

### Gated modules (category 1)

`api::pbt_infrastructure` is now gated behind
`#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "test-helpers")))]`.
The wasm exclusion is preserved (proptest doesn't compile for WASM). The
`test-helpers` clause prevents compilation in production native builds of `holon`.

To support this, `loro_backend_pbt.rs` (which imports from `pbt_infrastructure`)
moved from the featureless `api_suite` binary to a new `api_pbt` binary with
`required-features = ["test-helpers"]`. `api_suite` remains a zero-feature quick
suite by design.

The `test-helpers` feature contract remains: empty feature `[]`, zero default
dependencies, purely compile-time, never enabled by production crates. Cargo
feature unification means `cargo check --workspace` still compiles the module
(because `holon-integration-tests` enables the feature workspace-wide). The
boundary win is for production dependents built standalone
(`cargo check -p holon --release` etc.).

### Deliberate prod test-seams (category 2 — the "test seams are prod API" alternative)

`ReactiveEngineDriver` + `HeadlessEditorMirror` are **production API** for the
MCP-driver path. The TUI frontend's `UserDriver` wraps `ReactiveEngineDriver`
directly. Every frontend's MCP server drives UI interactions through a
`UserDriver`. These types live in `holon-frontend` unconditionally — no
`test-helpers` feature in `holon-frontend`.

The quiescence/drop-timeout env vars (`holon-frontend/src/user_driver.rs:945–957`)
and `HOLON_PBT_SUPERHUMAN_INPUT` (`frontends/gpui/src/user_driver.rs:905`) are
self-contained, default-off pacing knobs. They are read once at construction time and
have no effect unless explicitly set by a test harness. They are test seams in an
otherwise production type — not a boundary violation that needs gating.

### Harness configuration (category 4)

`HOLON_PBT_*` vars inside `holon-integration-tests` are test harness configuration,
not a boundary issue. That crate IS the test harness.

### Follow-up trigger

When a fourth env seam appears in a production path, migrate the env seams to
`HolonConfig` (holon.toml layering) as a proper configuration surface. Until then,
three env vars is not enough surface to justify the migration.

## Consequences

- `pbt_infrastructure` is no longer part of `holon`'s public API in production
  builds. Standalone production consumers (`-p holon --release`) do not compile it.
- `holon-frontend` does **not** receive a `test-helpers` feature — there is nothing
  to gate there (the types are production, and the env seams are self-contained).
- `holon-integration-tests` already enables `test-helpers` on `holon` — no change.
- The `api_pbt` binary requires `--features test-helpers` to run. Bare
  `cargo test -p holon` silently skips it (CI always passes the feature).
- A future `holon-test-support` crate is deferred until the gated surface is large
  enough to justify a new crate dependency in the graph — currently ~700 lines in
  one file.

## Gated module inventory (post-this-ADR)

| Module | Location | Gate |
|--------|----------|------|
| `storage::test_helpers` | `crates/holon/src/storage/` | `any(test, feature = "test-helpers")` |
| `di::test_helpers` | `crates/holon/src/di/` | `any(test, feature = "test-helpers")` |
| `e2e_test_helpers` | `crates/holon/src/testing/` | `any(test, feature = "test-helpers")` |
| `api::pbt_infrastructure` | `crates/holon/src/api/` | `all(not(wasm32), any(test, feature = "test-helpers"))` (this change) |
