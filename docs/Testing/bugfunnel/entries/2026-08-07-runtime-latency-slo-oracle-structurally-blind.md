---
id: 2026-08-07-runtime-latency-slo-oracle-structurally-blind
date: 2026-08-07
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The runtime `holon_oracles` latency-slo oracle is structurally blind to warm
  document switches.
source_line: 1182
---

## Bug

(task-#37 latency-gate lane, same session) **The runtime `holon_oracles`
latency-slo oracle is structurally blind to warm document switches.** It
judges `stage=e2e` events only. A warm switch re-shows an
already-materialized page, delivers no CDC rows, and so never closes a
correlator entry — it emits no e2e event at all, and the SLO banner cannot
fire on it no matter how slow the switch is. Empirical: 24 `NavigateFocus`
transitions per replay yield `navigate` n=1, and that one sample is a cold
materialization rather than a switch. The oracle exists and works (it fired
and banner-disclosed the `split_block` row above), but the only navigation
class it can observe is the cold tail; the common interaction — switching
between pages the user has already visited — is invisible to it.

## Root cause

the runtime `holon_oracles` latency-slo oracle is STRUCTURALLY BLIND to warm
document switches. It judges `stage=e2e` events only, and a warm switch
re-shows an already-materialized page, delivers no CDC rows, and therefore
never closes a correlator entry — so it emits no e2e event and the SLO
banner cannot fire on it regardless of how slow the switch is. Empirical: 24
NavigateFocus transitions per replay yield navigate n=1, and that one sample
is a cold materialization, not a switch. The oracle exists, works, and is
disclosure-only (see the 2026-08-07 split_block row), but the class of
navigation it can see is exactly the cold tail; the common interaction —
switching between pages the user already visited — is unobservable to it.
ORACLE by definition: the interaction generates fine and the invariant
cannot go red on it. Distinct from the row above, which is about a wrong
VALUE; this is about NO value existing. Missing piece is a decision, not
just a test: either warm switches should emit an e2e sample (they deliver no
rows by design, so this needs a different close condition) or navigation
needs an observable that is not row-delivery. NO FIX. Evidence:
`docs/Testing/fixture-logs-2026-08-07/latency-attribution.txt` FINDING 2
"SECOND-ORDER FINDING (oracle coverage)")

## Missing piece

ORACLE by definition: the interaction generates fine and no invariant can go
red on it. Distinct from the row above, which is a wrong VALUE; this is NO
value existing. Missing piece is a decision before a test: either warm
switches must emit an e2e sample (they deliver no rows by design, so this
needs a different close condition than row-delivery) or navigation needs an
observable that is not row-delivery at all. Until then any "navigation is
within SLO" claim from the runtime oracle is unfalsifiable for warm
switches.

## Remedy

OPEN 2026-08-07 — diagnosis only, no fix. ANNOTATED 2026-08-08 (task #13):
the decision this row asks for is now MADE and recorded in code, and it is
the conservative one — a warm switch delivers nothing, so it gets NO e2e
sample and expires LOUDLY as `stage="e2e_expired" action=navigate …
waited_ms=…` (pinned by
`a_navigation_that_delivers_nothing_expires_loudly`). Task #13 deliberately
did NOT invent a substitute close condition: the alternative it rejected —
letting a navigation close on whatever batch happens to name its page — is
the very defect it fixed (11982ms billed to a ~100ms navigation), so any
future warm-switch measurement must come from a NEW observable the pipeline
does not emit today (a render/paint-side signal for navigation), not from
relaxing the correlation. The blindness this row names therefore SURVIVES
the fix and is now narrower and explicit: warm switches are
unmeasured-and-disclosed rather than unmeasured-and-silent. Disclosure is
also timelier — `rows_delivered` prunes before matching, so an expiry
surfaces on the next delivered batch instead of the next dispatch. Evidence:
`docs/Testing/fixture-logs-2026-08-07/latency-attribution.txt` FINDING 2
"SECOND-ORDER FINDING (oracle coverage)"; ruling recorded in
`docs/Testing/fixture-logs-2026-08-08/task13-navigate-observable-red-green.txt`
and in the `latency_e2e` module docs.
