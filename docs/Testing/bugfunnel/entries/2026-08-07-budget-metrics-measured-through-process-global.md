---
id: 2026-08-07-budget-metrics-measured-through-process-global
date: 2026-08-07
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Budget metrics were measured through ONE process-global span buffer, so
  concurrent tests in a binary charged each other's SQL and wiped each other's
  windows.
source_line: 1176
---

## Bug

(found by a FLAKING GATE, not by any assertion — the gate that measures
every other gate) **Budget metrics were measured through ONE process-global
span buffer, so concurrent tests in a binary charged each other's SQL and
wiped each other's windows.** `just hand-authored` intermittently reported
8/9 with `echo_loop_block_to_page_child_render_leak_parked` failing
`CreateBlockUnderFocus.sql_ddl: 54-55 exceeds 0 + 42`, while that same test
PASSES in isolation and the 34-case corpus stays green.
`SpanCollector::global()` installed a single `InMemorySpanExporter` and
`finished_spans`/`reset` addressed it directly, so the corpus test's 34
DDL-heavy environment boots were charged to whichever transition window the
echo test happened to have open, and each test's per-transition `reset()`
erased the other's window. Both directions reproduced deterministically
before the fix: a bystander scope was charged 7 spans a concurrent scope
drove (`left: 7, right: 0`), and a concurrent scope's `reset` erased a span
this window had recorded (`left: 0, right: 1`).

## Root cause

secondary ENVIRONMENT: found by a FLAKING GATE, not by any assertion — `just
hand-authored` intermittently reported 8/9 with
`echo_loop_block_to_page_child_render_leak_parked` failing
`CreateBlockUnderFocus.sql_ddl: 54-55 exceeds 0 + 42`, while the same test
PASSES in isolation and the 34-case corpus stays green. ROOT CAUSE: every
budget number was measured through ONE process-global buffer.
`SpanCollector::global()` installed a single `InMemorySpanExporter`
(test_tracing.rs:371) and `finished_spans`/`reset` addressed it directly, so
`#[test]` fns running concurrently in one binary shared a single metrics
window — the corpus test's 34 environment boots (DDL-heavy) were charged to
whichever transition window the echo test happened to have open, and each
test's per-transition `reset()` WIPED the other's window. Both directions
were reproduced deterministically before the fix: a bystander scope was
charged 7 spans a concurrent scope drove (`left: 7, right: 0`), and a
concurrent scope's `reset` erased a span this window had recorded (`left: 0,
right: 1`). Nothing could have caught it: `inv-sql-budget` asserts on the
window's CONTENTS and no invariant asserted the window's ISOLATION, so the
oracle was fed the wrong input rather than reaching a wrong verdict — hence
ORACLE, not COVERAGE (the interaction generates fine) . Secondary
ENVIRONMENT because it is invisible in the one-test-per-binary wiring the
keystone runs under and only became hot when an unrelated ~100s corpus
speedup changed which tests overlap — the escape is timing-shaped, which is
why it survived every green gate for as long as the two tests happened not
to collide. FIXED: spans are now routed to the `TestScope` owning the
emitting thread, the same per-scope mechanism ERROR/panic capture already
used, so a window holds exactly its own work; spans no scope can be charged
for are counted and DISCLOSED per window as `UNATTRIBUTED-SPANS=` on the
budget line instead of silently shrinking a budget. That disclosure
immediately exposed a second, previously invisible measurement hole it now
reports as zero: `holon-frontend`'s three `std::thread::scope` bridge
threads (`reactive.rs` watch_query/list_templates/resolve_block, which exist
only to `block_on` outside a runtime) are anonymous to every thread-keyed
facility, and were dropping 12–24 spans in 76 of 166 budget windows — those
threads now inherit their spawner's context through a `bridge_thread` hook.
NO budget line, ceiling or tolerance was changed: parallel-vs-serialized
measurement differs by 128 normalized budget lines against a 122–128
same-build run-to-run noise floor, i.e. within noise)

## Missing piece

`inv-sql-budget` asserts on the window's CONTENTS; nothing asserted the
window's ISOLATION, so the oracle was fed the wrong input rather than
reaching a wrong verdict — the interaction generates fine, which is what
makes this ORACLE rather than COVERAGE. Secondary ENVIRONMENT because it is
structurally invisible in the one-test-per-binary wiring the keystone runs
under, and only became hot when an unrelated ~100s corpus speedup changed
which tests overlap — the escape is timing-shaped, which is why it survived
every green gate for as long as the two tests happened not to collide.
Missing piece = spans routed to the `TestScope` owning the emitting thread
(the per-scope mechanism ERROR/panic capture already used), plus a
disclosure for spans no scope can be charged for, so an under-measurement
can never be silent.

## Remedy

FIXED 2026-08-07 — routing landed with two red-first unit tests pinning both
directions. The new `UNATTRIBUTED-SPANS=` disclosure immediately exposed a
SECOND, previously invisible measurement hole it now reports as zero:
`holon-frontend`'s three `std::thread::scope` bridge threads (`reactive.rs`
watch_query/list_templates/resolve_block, which exist only to `block_on`
outside a runtime) are anonymous to every thread-keyed facility and were
dropping 12-24 spans in 76 of 166 budget windows; they now inherit their
spawner's context through a `bridge_thread` hook, after which 0 of 166
windows report loss. NO budget line, ceiling or tolerance was changed:
parallel-vs-serialized measurement differs by 128 normalized budget lines
against a 122-128 same-build run-to-run noise floor.
