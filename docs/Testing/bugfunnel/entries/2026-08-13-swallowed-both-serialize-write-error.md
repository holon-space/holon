---
id: 2026-08-13-swallowed-both-serialize-write-error
date: 2026-08-13
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  `RunResultGuard::finalize` swallowed both the serialize and the write error
source_line: 714
---

## Bug

(task-#11 observability-fix lane; found by reading the code during the OTel
perf research pass, `lane-logs/research-otel-perf.md`)
**`RunResultGuard::finalize` swallowed both the serialize and the write
error** (`crates/holon-integration-tests/src/pbt/run_result.rs:228-232`): an
unusable `HOLON_RESULT_DIR` produced no run record and no message, so the
observability report loses runs indistinguishably from runs never started.

## Root cause

task-#11 observability-fix lane, found by READING THE CODE during the OTel
perf research pass (`lane-logs/research-otel-perf.md`) — no test produced
it: **`RunResultGuard::finalize` swallowed BOTH the serialize error and the
write error**
(`crates/holon-integration-tests/src/pbt/run_result.rs:228-232`, `let _ =
std::fs::create_dir_all(...)` then `if let Ok(json) = ... { let _ =
std::fs::write(...) }`), so an unusable `HOLON_RESULT_DIR` produced NO run
record and NO message — the observability report would simply be missing
runs, indistinguishable from runs never started. The write path itself runs
in every keystone/GPUI run; only its error branch was unasserted, and the
two unit tests both passed a good tempdir, so nothing could ever go red.
COVERAGE secondary: no case ever supplied an unusable output dir. FIXED:
`write_record()` returns `anyhow::Result` with the directory/path in the
context, and `finalize` discloses on stderr AND via `tracing::error!` (it
cannot panic — Drop also runs while unwinding). Pinned by
`an_unwritable_dir_yields_an_enriched_error`; inverted by dropping the
`.with_context` → red on the missing path, "Not a directory (os error 20)"
alone.)

## Missing piece

The write path runs in every keystone/GPUI run but its error branch was
unasserted; both unit tests passed a good tempdir, so no case could go red.

## Remedy

FIXED — `write_record()` returns `anyhow::Result` carrying the directory and
the path; `finalize` discloses on stderr and via `tracing::error!` (never
panics: Drop also runs while unwinding). Pinned by
`an_unwritable_dir_yields_an_enriched_error`, inverted red by dropping
`.with_context`.
