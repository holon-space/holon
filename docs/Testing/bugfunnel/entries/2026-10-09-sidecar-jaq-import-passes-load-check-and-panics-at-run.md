---
id: 2026-10-09-sidecar-jaq-import-passes-load-check-and-panics-at-run
date: 2026-10-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A sidecar jaq mapping `import "x" as $x; $x` passed the load check, then panicked
  (`Option::unwrap()` on None in jaq-core) the first time it ran in the sync path.
---

## Bug
Found by the security verifier of the jaq allowlist change (`lane-logs/jaq-verify.md`, D1).
`RowMapper::compile` returned Ok; `RowMapper::map` panicked at jaq-core-3.1.1
`filter.rs:549`. `import "x" as $x; .` compiled and ran.

## Root cause
jaq's loader leaves a `$`-import to the embedder: it records the variable without calling the
module reader, and the compiler binds it as a global. `RowMapper::run`
(`crates/holon-rows/src/mapping.rs`) passes `Vars::new([])`, so the lookup of `$x` finds
nothing. Module imports and `include` were refused, but by jaq's default reader with the text
"module loading not supported", not a mapping-level reason. No other construct reaches that
lookup: `RowMapper` declares no global variables, and `$__loc__`, `$ENV` and `$__prog_args` are
undefined at load.

## Missing piece
No test compiled a mapping that imports or includes anything, and no test ran every accepted
mapping under `catch_unwind`.

## Remedy
`RowMapper::compile` refuses every `import` and `include`: a reader that refuses with
`jaq_library::IMPORT`, then `jaq_core::load::import` over the loaded modules for `$`-imports.
The error reads "import of `x` is not available to a mapping: <reason>". Tests
`a_mapping_the_load_check_accepts_does_not_panic_when_it_runs` and
`module_and_data_imports_are_refused_at_load_with_the_reason` in
`crates/holon-rows/tests/mapper_bounds.rs`. Red log `lane-logs/jaq-harden-r2-red-d1.log`;
green `lane-logs/jaq-harden-r2-green-d1.log`.
