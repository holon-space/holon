---
id: 2026-07-12-cargo-hakari-regenerate-reverted-workspace-hack
date: 2026-07-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  cargo-hakari regenerate reverted workspace-hack's cfg(not(wasm32)) gate —
  worker wasm build broken (tokio net via rmcp→oauth2→hyper)
source_line: 981
---

## Bug

cargo-hakari regenerate reverted workspace-hack's cfg(not(wasm32)) gate —
worker wasm build broken (tokio net via rmcp→oauth2→hyper)

## Missing piece

no CI builds the wasm targets; hakari output not asserted

## Remedy

FIXED (re-gated; commit 872dbf4a)
