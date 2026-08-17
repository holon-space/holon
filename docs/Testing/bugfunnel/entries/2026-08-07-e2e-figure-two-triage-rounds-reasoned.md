---
id: 2026-08-07-e2e-figure-two-triage-rounds-reasoned
date: 2026-08-07
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The `navigate` e2e figure that two triage rounds reasoned from is partly
  MEASUREMENT, not latency.
source_line: 1181
---

## Bug

(task-#37 latency-gate lane, deterministic replay) **The `navigate` e2e
figure that two triage rounds reasoned from is partly MEASUREMENT, not
latency.** `latency_e2e` closes a navigation's clock when any delivered CDC
row carries the target id — for navigation, a CHILD row with `parent_id =
target`. When those rows arrive late (a just-created or not-yet-materialized
page) the clock keeps running across every unrelated interaction in between.
Replaying 24 deterministic `NavigateFocus` transitions per run: each costs
p50 ~230ms / p95 ~265-360ms of harness `action_total` with `reads=0/26
writes=0/0 ddl=0/0` — no query work at all — and yields ZERO e2e samples;
the single e2e sample per run sits on an unmaterialized page at 789-1517ms.
Two measurements of navigation in ONE run disagree 5x (action_total p50
~230ms vs the e2e sample at 1194ms). The dogfooded p50 1364ms / p95 1391ms
therefore conflates a real ~230-360ms per-switch cost with clock bleed, and
its tight spread — read as proof of a deterministic per-switch cost — is
equally consistent with a systematic "close on whatever eventually touches
this page" rule.

## Root cause

found while building the task-#37 latency ratchet gate — the `navigate` e2e
number that two triage rounds reasoned from is partly MEASUREMENT, not
latency. `latency_e2e`'s navigation clock closes when any delivered CDC row
carries the target id, which for navigation means a CHILD row with
`parent_id = target`; for a page whose rows arrive late (a just-created or
not-yet-materialized document) that clock keeps running across every
unrelated interaction in between. Replaying 24 deterministic NavigateFocus
transitions per run: each costs p50 ~230ms / p95 ~265-360ms of harness
`action_total` with reads=0/26, writes=0/0, ddl=0/0 — no query work at all —
and produces ZERO e2e samples, while the one e2e sample per run lands on an
unmaterialized page at 789-1517ms. Two measurements of navigation in the
SAME run disagree 5x (action_total p50 ~230ms vs the e2e sample at 1194ms).
So the dogfooded p50 1364ms / p95 1391ms figure conflates a real ~230-360ms
per-switch cost with clock bleed, and its tight spread — read as proof of a
deterministic per-switch cost — is equally consistent with a systematic
"close on whatever eventually touches this page" rule. Classified ORACLE,
NOT PERCEPTION, per this skill's explicit carve-out that latency escapes are
ORACLE or ENVIRONMENT: the litmus "could any headless assertion express
this?" is YES and was demonstrated headlessly, and the correlator runs
identically in test and prod so no wiring or scale divergence makes it
ENVIRONMENT. The keystone generates navigations constantly; nothing asserts
that a closed e2e measurement is attributable to the interaction it is
charged to. NO FIX — reported so the fix lane re-measures before optimising
a 1.4s target that may be ~300ms. Evidence:
`docs/Testing/fixture-logs-2026-08-07/latency-attribution.txt` FINDING 2,
`.../nav-probe.txt` (same dir; recal runs summarized therein))

## Missing piece

NOT PERCEPTION, per this skill's explicit carve-out that latency escapes are
ORACLE or ENVIRONMENT: the litmus "could any headless assertion express
this?" is YES and was demonstrated headlessly; and the correlator runs
identically in test and prod, so no wiring or scale divergence makes it
ENVIRONMENT. The keystone generates navigations constantly and nothing
asserts that a closed e2e measurement is attributable to the interaction it
is charged to. Missing piece = an invariant cross-checking the two latency
measurements of one interaction (e.g. a closed e2e elapsed must not exceed
its own transition's wall time), which would have failed on the very first
mis-attributed sample.

## Remedy

**CLOSED 2026-08-08 by task #13** — the clock-bleed mechanism diagnosed here
is fixed at its root (see the 2026-08-08 dogfood row for the full fix note):
a navigation now closes ONLY on its own `focus_roots` delivery, and a
child/block row for the page can neither close it nor supersede it. The
diagnosis was exactly right; the missing level was that the navigate's own
delivery was not merely LATE but structurally invisible — `focus_roots` rows
carry no `id`/`parent_id`, the only columns the delivery reader looked at.
The proposed invariant (a closed e2e elapsed must not exceed its own
transition's wall time) is NOT built here and stays worth building; the
correlator-level equivalent now exists as unit pins
(`a_block_row_delivery_never_closes_a_pending_navigate` and siblings).
MEASUREMENT NOTE PRESERVED: every navigate figure taken before 2026-08-08 —
the dogfooded p50 1364ms / p95 1391ms above included — remains
MEASUREMENT-NOT-ESTABLISHED and must be re-measured on the fixed clock
before anything is optimised; the defensible per-switch number is still the
harness `action_total` ~230-360ms. Evidence:
`docs/Testing/fixture-logs-2026-08-07/latency-attribution.txt` FINDING 2,
`.../nav-probe.txt`; fix red/green
`docs/Testing/fixture-logs-2026-08-08/task13-navigate-observable-red-green.txt`.
