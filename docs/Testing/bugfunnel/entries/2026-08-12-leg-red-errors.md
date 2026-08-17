---
id: 2026-08-12-leg-red-errors
date: 2026-08-12
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  `just check-worker-wasm` leg 2 is RED with 13 errors in `holon-loro`
source_line: 719
---

## Bug

(task-#22 registry/wasm triage lane; found by running leg 2 STANDALONE after
leg 1 aborted the recipe; invisible to the discovering lane and to every
gate) **`just check-worker-wasm` leg 2 is RED with 13 errors in
`holon-loro`** — `error[E0432]: unresolved import iroh` /
`crate::iroh_sync_adapter` / `crate::share_enrollment` at
`crates/holon-loro/src/iroh_advertiser.rs`, plus 3 downstream `E0599 … for
type !`. `crates/holon-loro/src/lib.rs:51` declares `pub mod
iroh_advertiser;` with NO cfg while every symbol it imports is behind
`feature = "iroh-sync"` — the two cfg attributes above it both stack onto
`import_atomicity_probe`, so the module reads as covered and is not. Root
`Cargo.toml:206-210` documents that `default-features = false` deliberately
drops `iroh-sync` so the wasm worker "gets a loro stack with no iroh"; that
stack does not compile. Module dates to 2026-07-27; the 2026-07-29 "fix(ci):
… worker wasm re-export cfg" fixed this class and missed it.

## Root cause

task-#22 registry/wasm triage lane, found by RUNNING LEG 2 STANDALONE after
leg 1 aborted the recipe — invisible to the discovering lane and to every
gate: **`just check-worker-wasm` leg 2 is RED with 13 errors in
`holon-loro`** — `error[E0432]: unresolved import iroh` /
`crate::iroh_sync_adapter` / `crate::share_enrollment` at
`crates/holon-loro/src/iroh_advertiser.rs`, plus 3 downstream `E0599 … for
type !`. `crates/holon-loro/src/lib.rs:51` declares `pub mod
iroh_advertiser;` with NO cfg while every symbol it imports sits behind
`feature = "iroh-sync"`; the two cfg attributes above it both stack onto
`import_atomicity_probe`, so the module READS as covered and is not. This
breaks precisely the configuration the gate exists to protect — root
`Cargo.toml:206-210` documents that `default-features = false` deliberately
drops `iroh-sync` so "the wasm worker gets a loro stack with no iroh", and
that stack does not compile. Module dates to 2026-07-27 (enrollment
ceremony); `rtryxmxtoozs` (2026-07-29, "fix(ci): … worker wasm re-export
cfg") fixed this exact class and missed this module. THE MASKING IS ITSELF
THE FINDING: leg 1 runs under `set -euo pipefail`, so its failure aborts the
recipe and leg 2 never executes — the gate reports ONE red when there are
TWO, and the discovering lane reported exactly what the gate showed it. NOT
FIXED — triage lane; diagnosed only.)

## Missing piece

Same enforcement gap as the row above, PLUS a second-order gap that is the
sharper finding: leg 1 MASKS leg 2 — the recipe runs under `set -euo
pipefail`, so leg 1's failure aborts it and leg 2 never executes. The gate
reports ONE red when there are TWO, and the discovering lane correctly
reported what the gate showed. Missing piece = enforcement, and a recipe
that runs both legs before failing.

## Remedy

OPEN 2026-08-12 — diagnosed, NOT fixed (triage-only lane). Fix direction:
give `pub mod iroh_advertiser;` the same `#[cfg(all(feature = "iroh-sync",
not(all(target_arch = "wasm32", target_os = "unknown"))))]` its siblings
carry; separately, make the recipe run both legs before failing. Evidence:
`lane-logs/wasm-gate-leg2-native-test.log:657-863`.
