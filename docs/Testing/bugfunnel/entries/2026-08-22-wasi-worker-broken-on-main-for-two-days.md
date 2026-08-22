---
id: 2026-08-22-wasi-worker-broken-on-main-for-two-days
date: 2026-08-22
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The out-of-workspace wasi worker frontend did not compile on main for two
  days because the landing gate ran no recipe that builds it.
---

## Bug

`just check-worker-wasm` was red on `main` at 1417a5f26af7 — and had been since
the 08-20 loro-wiring refactor (535873b8d3e9). Found by the `repin` lane while
measuring an unrelated turso pin bump, not by any gate.

THREE independent failures, uncovered one behind the other — each one hid the
next, so the brief's single reported symptom was only the middle layer:

1. Lockfile staleness, which aborted the build before any compilation:
   `failed to select a version for the requirement turso_core = "*" (locked to
   0.8.0-pre.2) / candidate versions found which didn't match: 0.8.0-pre.3`.
   `frontends/holon-worker/Cargo.lock` recorded the turso packages at version
   `0.8.0-pre.2` while naming git rev `2f475750` as their source — and that rev
   actually contains `0.8.0-pre.3` (the root `Cargo.lock:14392` has it right).
   The worker's lock had been rev-string-edited without re-resolving.

2. `error[E0433]: cannot find module or crate holon_loro_wiring` —
   `frontends/holon-worker/src/lib.rs:306` calls
   `holon_loro_wiring::EventInfraModule`, but
   `frontends/holon-worker/Cargo.toml` declared no dependency on
   `holon-loro-wiring`. Reachable only after (1) was fixed.

3. Once the dependency existed, 11 × `error[E0433]: cannot find module or
   crate tokio`, all inside `holon-loro-wiring` itself
   (`loro_ui_watcher.rs:44,99,135,191,369`, `memory_backend.rs:32,76,143,180`,
   `loro_block_query_source.rs:38`, `loro_module.rs:31`). The crate had never
   been compiled for a wasm target by anything in the tree. Reachable only
   after (2) was fixed.

## Root cause

`frontends/holon-worker` is deliberately outside the root cargo workspace (it
declares its own empty `[workspace]` so resolver v2 keeps native-only deps out
of the wasm build) and carries its own tracked `Cargo.lock`. Consequences:

- `cargo check --workspace` cannot see it, so the 08-20 refactor that moved
  `EventInfraModule` out of `holon` into the new `holon-loro-wiring` crate
  updated every workspace call site and left the worker's call site to rot.
- The worker's lock is resolved by nothing else, so it drifted against the
  holon crates too (`holon-rules` and `holon-secrets` were never added).
- Nothing else in the tree builds `holon-loro-wiring` for a wasm target, so
  its manifest could put tokio under
  `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`
  (`crates/holon-loro-wiring/Cargo.toml:43-44`) while its source called
  `tokio::spawn` / `tokio::sync` / `tokio::time` unconditionally, and no build
  ever contradicted it. Feature unification cannot paper over this: an
  undeclared dependency is not linkable no matter what other crates enable.

The three defects were serial, not parallel — each masked the next — which is
why the escape looked like one error and was three.

`just check-worker-wasm` (justfile:568) is the recipe that does see it. It is
wired into `just precommit` and into `.github/workflows/ci.yml:146` — but it
was **not** a step of `just landing-gate` (justfile:1025), and the landing gate
is what lanes and the weave run before landing. Every landing since 08-20
therefore passed with the worker red.

## Missing piece

`landing-gate` had a step for the other out-of-workspace wasm crate
(`check-dioxus-web-wasm`) but none for the out-of-workspace wasi worker. The
keystone PBT cannot substitute: it never builds the worker's target at all, so
no invariant or generator change could have caught this — the failing code path
does not exist in the keystone's wiring. Hence ENVIRONMENT, not COVERAGE.

## Remedy

- `frontends/holon-worker/Cargo.toml`: added the `holon-loro-wiring` path
  dependency with `default-features = false` — its default `iroh-sync` feature
  enables `holon-loro/iroh-sync`, which is QUIC- and native-only. Verified
  against the worker's own target that the new edge drags no non-wasm dep in:
  `cargo tree --target wasm32-wasip1-threads` shows no `iroh`, `keyring`,
  `secret-service` or `zbus` in the wasm graph (`lane-logs/tree-wasm-full.log`).
- `frontends/holon-worker/Cargo.lock`: re-resolved via a *targeted*
  `cargo update -p turso_core …` (never a bare `cargo update`). The turso git
  rev is unchanged at `2f475750`; only the stale `0.8.0-pre.2` version strings
  became `0.8.0-pre.3`. The rest of the diff is holon-crate drift
  (`holon-rules`, `holon-secrets` and the natively-gated `keyring` family they
  pull). Nothing was removed except the nine stale `0.8.0-pre.2` turso entries;
  no ed25519/curve25519/crypto-common churn.
- `crates/holon-loro-wiring/Cargo.toml`: tokio moved into the main
  `[dependencies]` as
  `{ workspace = true, default-features = false, features = ["rt", "sync", "time", "macros"] }`
  — the wasm-safe set the repo already uses in `frontends/holon-worker` and in
  `crates/holon-loro/Cargo.toml:61-62`. The native `features = ["full"]` line
  in the `cfg(not(wasm32))` block is untouched; cargo unions the two, so
  native builds are unchanged.
- `justfile`: `check-worker-wasm` is now landing-gate step 5 of 8 — the rung
  that closes this gap. Separately, every gate/check recipe's log moved off its
  fixed `/tmp/*.log` path onto per-workspace `target/gate-logs/<same
  basename>` (57 lines across 35 recipes), because parallel lanes were
  overwriting one another's verdict in the shared files — one lane was observed
  reading another workspace's build as its own. The recipe's `set -euo pipefail` was verified
  to carry cargo's exit code through the `| tee` — proven with a scratch
  replica whose no-pipefail control exits 0 while the armed form exits 1 — so
  the new gate step cannot pass vacuously.
