---
id: 2026-10-09-sidecar-jaq-mapping-can-exit-holon-and-read-its-environment
date: 2026-10-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A sidecar's jaq mapping could call `halt`/`halt_error` (exits the Holon process), `env`
  (reads every process environment variable, secrets included), `debug`/`stderr` (writes the
  log) and `now`/`localtime` (non-deterministic rows).
---

## Bug
Found by a code audit of the jaq response mappers (vault item `5cbf9b32`, findings F1, F6,
F7). A mapping such as `if .version == null then "x" | halt_error else . end` ended the whole
process when a peer omitted `version`; `{x: env}` in a request mapping sent the environment to
the peer.

## Root cause
`RowMapper::compile` (`crates/holon-rows/src/mapping.rs`) handed every mapping the whole of
`jaq_std::funs()`/`defs()`, and `RowMapper::run` passed each result through
`jaq_core::unwrap_valr`, which calls `std::process::exit` on a halt.

## Missing piece
No test compiled or ran a mapping that calls a process, environment, clock or log builtin.

## Remedy
`crates/holon-rows/src/jaq_library.rs` offers only the names in `ALLOWED`; `WITHHELD` names
the refused ones and the reason a compile error states. `run` maps a non-error exception to
`Err`. Tests: `crates/holon-rows/tests/mapper_bounds.rs` (halt and env run in a child
process), `jaq_library::tests`, `crates/holon-mcp-client/tests/bundled_mappings_compile.rs`.
Red log `lane-logs/jaq-harden-red-rows.log`; teeth `lane-logs/jaq-harden-teeth-allowlist.log`.
