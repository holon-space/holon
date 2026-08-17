---
id: 2026-08-13-named-file
date: 2026-08-13
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  `maybe_write_flamegraph` named every file `{transition_key}.folded`
source_line: 715
---

## Bug

(task-#11 observability-fix lane; found by reading the code during the OTel
perf research pass, `lane-logs/research-otel-perf.md`)
**`maybe_write_flamegraph` named every file `{transition_key}.folded`**
(`crates/holon-integration-tests/src/test_tracing.rs:1452`) while being
called once per transition
(`crates/holon-integration-tests/src/pbt/sut_metrics.rs:384`) with a
repeating key — each write erased the previous one, leaving one file per key
holding the LAST visit rather than the heaviest.

## Root cause

task-#11 observability-fix lane, found by READING THE CODE during the OTel
perf research pass (`lane-logs/research-otel-perf.md`) — no test produced
it: **`maybe_write_flamegraph` named every file `{transition_key}.folded`**
(`crates/holon-integration-tests/src/test_tracing.rs:1452`), and it is
called once per transition
(`crates/holon-integration-tests/src/pbt/sut_metrics.rs:384`) with a key
that repeats across the run — so each write ERASED the previous one and a
whole perf run left exactly one folded-stacks file per key, silently the
LAST visit rather than the heaviest. No automated run sets
`HOLON_PERF_FLAMEGRAPH`, so the writer executes in no gate at all; ORACLE
secondary — nothing asserts the output files are distinct. FIXED:
`folded_file_name()` prefixes a process-monotonic write counter and the pid.
Pinned by `repeated_writes_for_one_key_keep_both_files`; inverted back to
the bare key → red on two identical paths.)

## Missing piece

No automated run sets `HOLON_PERF_FLAMEGRAPH`, so the writer executes in no
gate; and nothing asserts the output files are distinct.

## Remedy

FIXED — `folded_file_name()` prefixes a process-monotonic write counter and
the pid. Pinned by `repeated_writes_for_one_key_keep_both_files`, inverted
red against the bare key.
