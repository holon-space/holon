---
id: 2026-08-12-leg-tier-step-red
date: 2026-08-12
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  `just check-worker-wasm` leg 1 — Tier-1 `precommit` step 3/3 — is RED on
  `wasm32-wasip1-threads`: `error[E0433]: cannot find fs_port in crate`
source_line: 718
---

## Bug

(task-#22 registry/wasm triage lane; originally found by the
deterministic-scheduling lane; no gated test produced it) **`just
check-worker-wasm` leg 1 — Tier-1 `precommit` step 3/3 — is RED on
`wasm32-wasip1-threads`: `error[E0433]: cannot find fs_port in crate`** at
`crates/holon-filesystem/src/sync_base_store.rs:167`. Two cfg predicates
meant to agree do not: `fs_port` is gated `not(target_arch = "wasm32")`
(`lib.rs:23-24`, absent on ALL wasm) while the native `persist` calling it
is gated `not(all(target_arch = "wasm32", target_os = "unknown"))` (`:139`)
— TRUE on wasip1, whose `target_os` is `wasi`. The wasm stub `persist`
(`:175`) is gated `all(wasm32, unknown)` and does not cover wasip1 either,
so the target falls between the two arms and compiles the native body
without the module. Introduced 2026-08-08 by the atomic-sidecar landing
(task #24 / ADR 0030 D3.1).

## Root cause

task-#22 registry/wasm triage lane, found by RUNNING Tier-1 `precommit`'s
wasm step directly (originally by the deterministic-scheduling lane) — no
gated test verdict produced it: **`just check-worker-wasm` leg 1 is RED on
`wasm32-wasip1-threads`: `error[E0433]: cannot find fs_port in crate`** at
`crates/holon-filesystem/src/sync_base_store.rs:167`. Two cfg predicates
that were meant to agree do not: `fs_port` is gated `not(target_arch =
"wasm32")` (`lib.rs:23-24`, absent on ALL wasm) while the native `persist`
that calls it is gated `not(all(target_arch = "wasm32", target_os =
"unknown"))` (`sync_base_store.rs:139`), which is TRUE on wasip1 because
that target's `target_os` is `wasi`, not `unknown`. The wasm stub `persist`
(`:175`) is gated `all(wasm32, unknown)` and does not cover wasip1 either,
so the target falls BETWEEN the two arms and gets the native body without
the module. Introduced 2026-08-08 by the atomic-sidecar landing (task #24 /
ADR 0030 D3.1), which added the `write_atomic_blocking` call under the wrong
one of the two predicates. NOT FIXED — triage lane; diagnosed only.)

## Missing piece

The gate that catches this EXISTS and is wired into `precommit`
(`justfile:833`), but `precommit` is not enforced per-landing and CI cannot
reach it (200/200 CI failures — see `docs/Testing/KeystoneKnownReds.md`
"Where it runs"). Missing piece = gate ENFORCEMENT, not coverage.

## Remedy

OPEN 2026-08-12 — diagnosed, NOT fixed (triage-only lane). Fix direction:
make the two predicates ask the same question; narrowing `persist`'s gate to
`not(target_arch = "wasm32")` is the smaller change and matches `fs_port`'s
stated intent. Evidence: `lane-logs/wasm-gate.log:591-608`.
