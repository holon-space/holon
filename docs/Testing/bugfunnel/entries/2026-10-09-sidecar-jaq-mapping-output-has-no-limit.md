---
id: 2026-10-09-sidecar-jaq-mapping-output-has-no-limit
date: 2026-10-09
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  A sidecar's jaq mapping could emit an unbounded stream of values (`repeat(1)`), which
  `RowMapper::run` buffered until memory ran out.
---

## Bug
Found by a code audit of the jaq response mappers (vault item `5cbf9b32`, finding F4).

## Root cause
`RowMapper::run` (`crates/holon-rows/src/mapping.rs`) collected every output into a `Vec`
with no count or size limit.

## Missing piece
No test ran a mapping whose output exceeds what a list snapshot emits.

## Remedy
`run` refuses a stream past `MAX_MAPPING_OUTPUTS` values or `MAX_MAPPING_OUTPUT_BYTES` of
JSON, naming the cap and the mapping. Tests in `crates/holon-rows/tests/mapper_bounds.rs`;
red log `lane-logs/jaq-harden-red-rows.log`. Open: a single value built inside jaq
(`[repeat(1)]`) and a filter that never yields stay unbounded until the CPU/memory bound
(D-jaq-bound) is ruled.
