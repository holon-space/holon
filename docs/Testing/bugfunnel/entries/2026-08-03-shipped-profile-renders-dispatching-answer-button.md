---
id: 2026-08-03-shipped-profile-renders-dispatching-answer-button
date: 2026-08-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The shipped `pending_question` profile
  (`docs/integrations/claude-history.yaml:162-165`) renders a dispatching
  answer button for EVERY question row, but the real provider refuses any
  answer to a non-head question — only `question_index == 0` has `answerable =
  1`, and `compose_answer` rejects non-head as its second refusal. A user
  clicking a later question's button always gets a provider refusal: correct
  fail-loud, misleading affordance. The chat-input strategy's enablement table
  (§7) already prescribes disabling the affordance from the row's `answerable`
  column via the sidecar `precondition` Rhai expression — declaratively, no
  Rust.
source_line: 1157
---

## Bug

(verifier audit during the answer_question contract fix) The shipped
`pending_question` profile (`docs/integrations/claude-history.yaml:162-165`)
renders a dispatching answer button for EVERY question row, but the real
provider refuses any answer to a non-head question — only `question_index ==
0` has `answerable = 1`, and `compose_answer` rejects non-head as its second
refusal. A user clicking a later question's button always gets a provider
refusal: correct fail-loud, misleading affordance. The chat-input strategy's
enablement table (§7) already prescribes disabling the affordance from the
row's `answerable` column via the sidecar `precondition` Rhai expression —
declaratively, no Rust.

## Root cause

verifier audit of the answer_question contract fix — the shipped
`pending_question` profile renders a DISPATCHING answer button for EVERY
question row, but the provider refuses any answer to a non-head question
(`answerable = 0`, `compose_answer` refusal #2); clicking such a button
always dies at the provider; no test renders more than one pending question
or clicks a non-head button)

## Missing piece

No test renders MORE THAN ONE pending question, so the non-head shape is
never composed and no assertion relates the rendered button set to the
`answerable` column. Missing: a `pending_question_render` case with two
questions asserting the non-head row renders without a dispatching wiring
(or with a disabled affordance) once the precondition ships.

## Remedy

OPEN 2026-08-03 — diagnosis only; fix = one `precondition: answerable == 1`
line on the answer tool config + the two-question render test.
