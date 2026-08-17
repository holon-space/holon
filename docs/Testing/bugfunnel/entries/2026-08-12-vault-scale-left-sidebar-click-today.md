---
id: 2026-08-12-vault-scale-left-sidebar-click-today
date: 2026-08-12
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  At vault scale a left-sidebar click on TODAY'S JOURNAL day-page never
  applies its `navigation.focus` intent — the click is DEAD, and it costs
  ~5.3s before silently falling back to a bare focus set.
source_line: 1199
---

## Bug

(D2 lane, investigating Martin's report of multi-second delays while ONLY
clicking the left sidebar, no editing; reproduced in two sandboxes driven
interleaved in one wall-clock window) **At vault scale a left-sidebar click
on TODAY'S JOURNAL day-page never applies its `navigation.focus` intent —
the click is DEAD, and it costs ~5.3s before silently falling back to a bare
focus set.** The corpus axis is the whole effect and this particular path is
a PURE READ: identical page, identical binary, `operation` 23->23 and
`block` 2055->2055 across five consecutive clicks. **CORRECTION, measured
after this row was first written and load-bearing — "sidebar clicks are
writes" is REFUTED ONLY FOR ROW CLICKS, and is TRUE for CHEVRON clicks:**
the sidebar disclosure triangle's `on_mouse_down`
(`frontends/gpui/src/render/builders/tree_item.rs:222-241`) dispatches
`OperationIntent::set_field(row, "collapsed", ...)` through the normal op
path, deliberately, so the fold is undoable and synced. Driving that exact
intent writes **one `operation` row per click, proven by an operation-count
delta of exactly 1 on all 12 measured clicks**, and costs **643ms settled at
2055 blocks vs 522ms at 210** (op-accept alone 366 vs 249). So a user "only
clicking around the sidebar, not editing" IS issuing `set_field` writes
whenever they expand or collapse a tree item — this is the tie between the
user-visible no-editing report and the `set_field` family. Note the chevron
cost is only ~1.2x per 10x corpus, i.e. it is over SLO at EVERY corpus size
rather than corpus-driven. Interleaved paired measurement, 4 rounds: journal
day-page **263ms mean at 210 blocks vs 5305ms at 2055 blocks** (p50 250 vs
5312, max 335 vs 5355) — a ~20x blowup on a ~10x corpus — while ORDINARY
sidebar pages are FLAT across the same axis (353ms at 210 vs 392ms at 2055),
confirming the prior lane's "navigation does not scale" result and
localizing the defect to one row rather than to navigation. MECHANISM,
proven by the app's OWN instrumentation rather than by wall time: at 210
blocks the journal block emits 4 `holon_latency stage="e2e" action=navigate`
records (the intent resolves and is applied); at 2055 blocks it emits
**ZERO** across 10+ clicks while ordinary pages in the SAME instance emit
theirs normally — so at scale the sidebar row binds no click intent,
`ReactiveEngineDriver::click_entity_with_modifiers`
(`crates/holon-frontend/src/user_driver.rs:769-800`) polls
`snapshot_resolved` until its 2s deadline (overshooting to ~5.3s because
each snapshot is itself slow at that corpus), and then falls through to
`set_focus` with no navigate and NO disclosure — the forbidden tier-4
"silently degrades to look fine". This is the `sidebar-focus-bind` known-red
family
(`crates/holon-integration-tests/tests/sidebar_bind_latency_probe.rs`)
reproducing in PRODUCTION as a scale-dependent dead affordance. SECOND,
INDEPENDENT SLO ROW IN THE SAME DATA: the app's own `e2e` for an ORDINARY
navigate at 2055 blocks reads 919/979/1165ms and raises `ORACLE VIOLATION:
[latency-slo] ... (SLO: p95 <200ms)` on every single one — so the flat
~335ms MCP wall figure UNDERSTATES the real interaction cost by ~3x (the MCP
`click` returns before projection is visible) and even the flat path is 5x
over budget. DISCLOSED, load-bearing for whoever fixes this: the 2s poll
deadline is a TEST-DRIVER construct, so the 5.3s wall figure is an
MCP-driver amplification of the underlying defect and is NOT the number a
GPUI user experiences — in the real window the click would be dead-but-fast;
the production-faithful claims are the ZERO e2e records and the 919-1165ms
ordinary-navigate e2e. Also NOT done: no cold-restart control at 2055
blocks, so "the instance degraded during its 6-minute ingest" is not
formally excluded (argued unlikely — ordinary pages in the same instance
stayed fast and correct throughout).

## Missing piece

the keystone runs at toy corpus size, so a bind that only fails past ~2000
blocks cannot occur in the test environment at all — no rung draws corpus
size as an axis; secondary ORACLE and independently damning: nothing asserts
that a sidebar click ACTUALLY APPLIES a navigate intent, so the bare-focus
fallback masks a dead click as a successful one, and no per-interaction
latency invariant fires in a one-shot sweep
(`inv-settle-budget`/`inv-sql-budget` are class-3 and skipped)

## Remedy

OPEN — P1, triage only, NO FIX in this lane. The D2 lane was funded for the
`set_field` edit-path dominator; this is a DIFFERENT and higher-priority
defect than the one the lane was scoped to, and re-pointing the fix budget
is Martin's call. Recommended remedy, in order: (1) make the bare-focus
fallback FAIL LOUD or at minimum log at WARN so a dead click stops looking
like a live one; (2) draw corpus size as an axis on the sidebar-bind probe
so the bind failure is reproducible headlessly; (3) then fix the
row-set/bind maintenance itself. Evidence: `lane-logs/paired-corpus.log`
(the paired table), `lane-logs/e2e-scale.txt`,
`/tmp/holon-d2-scale/logs/app.log` + `/tmp/holon-d2-small/logs/app.log`.
