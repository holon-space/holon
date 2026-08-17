---
id: 2026-07-30-sidebar-disclosure-state-has-machine-readable
date: 2026-07-30
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The sidebar disclosure state has no machine-readable observable, so neither
  its correctness nor its latency can be checked outside the pixels. (a)
  `describe_ui` does not reflect collapse: with `block:alpha-project` and
  `block:journals` at `collapsed=1` and their subtrees demonstrably hidden on
  screen, every descendant was still present in the `describe_ui` tree, and no
  `collapsed` / chevron / halo field appears on any tree_item — collapse
  filtering happens at the collection level, below what `describe_ui` dumps.
  An MCP-driven agent or rung cannot tell collapsed from expanded. (b) the
  toggle emits NO `holon_latency` stage: `scripts/measure_latency.py` on the
  session log reports only `rows` and `boot_*` — no `dispatch`, no `e2e` — so
  the p95 interaction→projection-visible SLO is unmeasurable for this
  interaction by the sanctioned tool. Stopwatch fallback over 12 toggles: p50
  103ms / p95 226ms against a measured 95ms MCP-harness floor (10 trivial
  calls, 91–100ms), i.e. ~5–10ms of real app work — NO SLO breach, but the
  number comes from a hand-rolled harness rather than from budget
  instrumentation.
source_line: 1122
---

## Bug

The sidebar disclosure state has no machine-readable observable, so neither
its correctness nor its latency can be checked outside the pixels. (a)
`describe_ui` does not reflect collapse: with `block:alpha-project` and
`block:journals` at `collapsed=1` and their subtrees demonstrably hidden on
screen, every descendant was still present in the `describe_ui` tree, and no
`collapsed` / chevron / halo field appears on any tree_item — collapse
filtering happens at the collection level, below what `describe_ui` dumps.
An MCP-driven agent or rung cannot tell collapsed from expanded. (b) the
toggle emits NO `holon_latency` stage: `scripts/measure_latency.py` on the
session log reports only `rows` and `boot_*` — no `dispatch`, no `e2e` — so
the p95 interaction→projection-visible SLO is unmeasurable for this
interaction by the sanctioned tool. Stopwatch fallback over 12 toggles: p50
103ms / p95 226ms against a measured 95ms MCP-harness floor (10 trivial
calls, 91–100ms), i.e. ~5–10ms of real app work — NO SLO breach, but the
number comes from a hand-rolled harness rather than from budget
instrumentation.

## Root cause

the sidebar disclosure state has NO machine-readable observable, so neither
its correctness nor its latency is checkable from outside the pixels. (a)
`describe_ui` does not reflect collapse: with `block:alpha-project` and
`block:journals` at `collapsed=1` and their subtrees demonstrably hidden on
screen, all of their descendants were still present in the `describe_ui`
tree, and no `collapsed` / chevron / halo field appears on any tree_item —
an agent or MCP-driven rung cannot distinguish collapsed from expanded. (b)
the toggle emits no `holon_latency` stage at all (`measure_latency.py`
reports only `rows` and `boot_*`), so the p95 interaction→projection-visible
SLO is unmeasurable for this interaction by the sanctioned tool. Stopwatch
fallback over 12 toggles: p50 103ms against a 95ms MCP-harness floor, i.e.
~5–10ms of real app work — no SLO breach, but that number came from a
hand-rolled harness, not from the budget instrumentation. ORACLE on both
counts: the interactions are perfectly generatable, nothing would go red.)

## Missing piece

A collapse observable on the `describe_ui` tree_item (the `collapsed` flag
and the chevron/halo the row actually painted), plus a
`set_field(collapsed)` `holon_latency` dispatch/e2e stage so the 200ms
budget has an input for this interaction.

## Remedy

FIXED-in-same-land 2026-07-30 — both observables land in the SAME change as
the affordance: `describe_ui` now emits `collapsed` on every `tree_item`
node (`ViewKind::TreeItem`, snapshotted from the row's `expanded` handle; a
leaf is never collapsed), and the toggle emits its `holon_latency` stage so
the 200ms budget has an input. No SLO breach was observed; the gap was that
a breach would have been invisible.
