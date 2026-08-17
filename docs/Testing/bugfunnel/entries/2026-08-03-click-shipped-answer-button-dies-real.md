---
id: 2026-08-03-click-shipped-answer-button-dies-real
date: 2026-08-03
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Every click on a shipped answer button dies at the real provider: the
  `pending_question` profile ships `action: answer_question()` with no param
  override, `question_options` dispatches a single scalar `label`
  (`crates/holon-frontend/src/shadow_builders/question_options.rs:113` default
  `label_param = "label"`, `:50` inserts `Value::String(label)`), but the
  binary requires `answers` as an ARRAY of option labels (`server.rs:509-514`)
  and rejects everything else with `cannot answer: answers must be an array of
  option labels`. Same escape mechanism as the send_message row above: the
  mock was authored from the sidecar, so mock+sidecar+UI were self-consistent
  against a contract the binary does not implement.
source_line: 1156
---

## Bug

(verifier audit during the send-contract fix lane, reading the real
`claude-code-history-mcp` source read-only) Every click on a shipped answer
button dies at the real provider: the `pending_question` profile ships
`action: answer_question()` with no param override, `question_options`
dispatches a single scalar `label`
(`crates/holon-frontend/src/shadow_builders/question_options.rs:113` default
`label_param = "label"`, `:50` inserts `Value::String(label)`), but the
binary requires `answers` as an ARRAY of option labels (`server.rs:509-514`)
and rejects everything else with `cannot answer: answers must be an array of
option labels`. Same escape mechanism as the send_message row above: the
mock was authored from the sidecar, so mock+sidecar+UI were self-consistent
against a contract the binary does not implement.

## Root cause

verifier audit during the send-contract fix — the shipped answer buttons are
broken identically: `question_options` binds a single scalar `label` param
(`crates/holon-frontend/src/shadow_builders/question_options.rs:113,:50`)
and the shipped `pending_question` profile ships `action: answer_question()`
with no override, while the real binary requires `answers: array<string>`
(`server.rs:509-514`, rejection `cannot answer: answers must be an array of
option labels`); the mock encoded the sidecar's invented `label` contract,
so no automated layer ever compared it to the binary)

## Missing piece

Same missing piece as the send row: no automated layer compares the
mock/sidecar contract to the real binary's declared schema (`tools/list`);
the generic contract check remains OPEN there.

## Remedy

FIXED 2026-08-03 (answer-contract lane). Mock re-authored FROM the BINARY,
red-first: `crates/holon-mcp-mock/src/lib.rs` publishes the provider's
declared `answer_question` schema (`answer_tool_schema`: `question_id`
string + `answers` array<string> minItems 1) and enforces it
(`check_answer_contract`), including `steer::compose_answer`'s own rules —
non-string element, empty selection, unanswerable non-head question,
non-offered label, a label containing `", "` (which the join cannot
express), duplicate selection — each with the binary's VERBATIM rejection
text, and it records the `", "`-joined text the dialog would store. RED:
both dispatching tests failed with the exact prod string `cannot answer:
answers must be an array of option labels` (log:
`lane-answercontract/red-mock.log`). GREEN, one seat:
`crates/holon-frontend/src/shadow_builders/question_options.rs` — the widget
param is now `answers_param` (default `"answers"`, the provider's name) and
each button binds `Value::Array([label])`, a one-element array per click;
the offered-set guard follows the rename and accepts an array or a scalar in
an explicit `answers:` override (`forced_labels`) so neither shape can force
a label the question never offered. No yaml change was needed: the shipped
`action: answer_question()` picks up the corrected default.
Once-only/intent-key semantics untouched. Regression guards:
`a_scalar_label_is_refused_by_the_provider_contract`,
`several_labels_are_recorded_comma_joined` (holon-mcp-mock),
`pending_question_renders_one_button_per_offered_option` now asserts the
ARRAY shape per button (holon-frontend). STILL OPEN, tracked on the send
row: the generic contract check that diffs every sidecar-declared tool
against the provider's advertised `tools/list` — two tools are now pinned by
hand, the class is not closed. Also open: multi-select questions
(`pending_question.multi_select = 1`) still render one single-answer button
per option, so a several-labels answer is expressible at the provider and in
the mock but not yet in the UI.
