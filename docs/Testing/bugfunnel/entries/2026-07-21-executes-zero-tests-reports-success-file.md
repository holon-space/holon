---
id: 2026-07-21-executes-zero-tests-reports-success-file
date: 2026-07-21
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  `cargo nextest run -p holon-orgmode` executes ZERO tests from
  `sync_controller_mutation_pbt.rs` and reports SUCCESS — the file is
  `#![cfg(feature = "di")]` and `di` is not a default feature. Compounding: a
  plain `--features di` run fail-fasts after 4 failures and silently SKIPS ~38
  tests including the widened PBT, so filtered runs read as green. Live
  instance of the known required-features blindspot; surfaced while
  red-firsting the Directory panic.
source_line: 1070
---

## Bug

`cargo nextest run -p holon-orgmode` executes ZERO tests from
`sync_controller_mutation_pbt.rs` and reports SUCCESS — the file is
`#![cfg(feature = "di")]` and `di` is not a default feature. Compounding: a
plain `--features di` run fail-fasts after 4 failures and silently SKIPS ~38
tests including the widened PBT, so filtered runs read as green. Live
instance of the known required-features blindspot; surfaced while
red-firsting the Directory panic.

## Missing piece

test-invocation gating: the crate's PBT surface is invisible to the default
command; needs `--features di --no-fail-fast` (or a default-on feature / CI
gate asserting non-zero test count)

## Remedy

OPEN (documented; gate wiring not yet changed)
