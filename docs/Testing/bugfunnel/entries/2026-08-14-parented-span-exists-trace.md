---
id: 2026-08-14-parented-span-exists-trace
date: 2026-08-14
gap: ORACLE
secondary: null
status: UNCLASSIFIED
summary: >-
  Every `live_data.apply_batch` was re-parented to a span id that exists in no
  trace
source_line: 716
---

## Bug

(instrumentation-holes lane, task #15; found by an agent reading the CDC
provenance path while auditing span connectivity; no automated test produced
it, and the covering assertion was green over the defect the whole time)
**Every `live_data.apply_batch` was re-parented to a span id that exists in
no trace**, so a mirror apply looked connected to the write that caused it
and could not be followed to it.
`ChangeOrigin::extract_trace_context_from_current_span`
(`crates/holon-api/src/streaming.rs:128`) took `trace_id` from the OTel
context but `operation_id` from the **`tracing` registry Id**
(`span.id().into_u64()`); the two are consumed as one value, since
`to_batch_trace_context()` feeds `operation_id` to `set_parent` as a W3C
parent span id. A registry Id parses as valid 16-hex, so nothing rejected it
— observed parents `0018000000000001` and `0128000000000007` against real
exporter ids like `1547a0a5f6a6f792`.

## Root cause

instrumentation-holes lane (task #15), found by an AGENT READING the CDC
provenance path while auditing span connectivity — no automated test
produced it, and the covering assertion was GREEN over the defect the whole
time: **every `live_data.apply_batch` was re-parented to a span id that
exists in no trace, so the mirror apply looked connected to the write that
caused it and was unfollowable.**
`ChangeOrigin::extract_trace_context_from_current_span`
(`crates/holon-api/src/streaming.rs:128`) took `trace_id` from the OTel
context but `operation_id` from the **`tracing` registry Id**
(`span.id().into_u64()`). Those two halves are consumed as ONE value:
`to_batch_trace_context()` puts `operation_id` into
`BatchTraceContext.span_id`, which `LiveData::subscribe` hands to
`set_parent` as a W3C parent. A registry Id parses as a valid 16-hex span
id, so nothing rejected it — measured parents looked like `0018000000000001`
and `0128000000000007` (registry index/generation packed into a u64) against
real exporter ids like `1547a0a5f6a6f792`. ORACLE by the skill's litmus,
taken in order. Coverage: NO — `interaction_trace_connectivity` already
drove exactly this interaction on every run. Environment: NO — the defective
line executed in-test, in the same wiring, and its output was collected.
Perception: NO — nothing visual. What was missing is an INVARIANT, and the
reason is precise: the existing assertion was `applies.iter().all(|s|
s.parent_span_id != SpanId::INVALID)` — it checked the parent was SET, never
that it RESOLVED, and a fabricated id satisfies "set" perfectly. This is the
same shape as an oracle that compares a field's presence instead of its
meaning. Missing piece: an assertion on the stamping path itself. FIXED in
this lane: both halves now come from the OTel span context.
Red-for-the-right-reason by INVERSION — restoring the registry-Id derivation
reddens the new assertion with `_change_origin would carry 0018000000000001
as the parent of every mirror apply, but the writing span's OTel id is
1547a0a5f6a6f792` (`lane-logs/instr-holes-INVERTED-RED-46650.log`); note the
FIRST inversion attempt PASSED because the probe targeted
`BatchTraceContext::from_current_span`, a sibling that was always correct —
the assertion only bites when it probes
`ChangeOrigin::local_with_current_span`, which is the path that actually
stamps the column.)

## Missing piece

Not coverage (`interaction_trace_connectivity` drove this exact interaction
every run) and not environment (the defective line ran in-test and its
output was collected). The assertion was `applies.iter().all(

## Remedy

s | s.parent_span_id != SpanId::INVALID)` — it checked the parent was SET,
never that it RESOLVED, and a fabricated id satisfies "set" perfectly.
Missing piece: an assertion on the stamping path itself, not on the shape of
its output. | FIXED 2026-08-14 — both halves now come from the OTel span
context. RED by inversion: restoring the registry-Id derivation reddens the
new assertion with `_change_origin would carry 0018000000000001 as the
parent of every mirror apply, but the writing span's OTel id is
1547a0a5f6a6f792` (`lane-logs/instr-holes-INVERTED-RED-46650.log`). The
FIRST inversion attempt passed, which is the sharper lesson: the probe
targeted `BatchTraceContext::from_current_span`, a correct sibling — the
assertion only bites when it probes `ChangeOrigin::local_with_current_span`,
the path that actually stamps the column.
