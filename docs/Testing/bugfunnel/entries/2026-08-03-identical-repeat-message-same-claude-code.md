---
id: 2026-08-03-identical-repeat-message-same-claude-code
date: 2026-08-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An identical repeat message to the same Claude Code session ("yes" … later
  "yes" again) is PERMANENTLY refused: `deterministic_intent_key`
  (`crates/holon-api/src/effect_id.rs`) fingerprints
  connector/tool/target/params — and params include the message text — with no
  per-submission component, so the once-only intent gate treats a genuinely
  new submission as a retry of the already-consumed one; the first send's
  pending-write record shadows every later identical text forever. Fixed by a
  per-submission `ComposeId` (v4 UUID) minted on the input node, folded into
  the intent key as an explicit 5th component, re-minted only where
  `Delivery::Proven` clears the draft — retries (incl. `approve()` replay of
  stored params) keep their id, a new submission after a proven send gets a
  fresh one; `_`-prefixed params are stripped at the operation-write dispatch
  so the id never crosses the connector boundary.
source_line: 1150
---

## Bug

(agent exploration, chat-input I0–I5 build-out; ruled and fixed same day) An
identical repeat message to the same Claude Code session ("yes" … later
"yes" again) is PERMANENTLY refused: `deterministic_intent_key`
(`crates/holon-api/src/effect_id.rs`) fingerprints
connector/tool/target/params — and params include the message text — with no
per-submission component, so the once-only intent gate treats a genuinely
new submission as a retry of the already-consumed one; the first send's
pending-write record shadows every later identical text forever. Fixed by a
per-submission `ComposeId` (v4 UUID) minted on the input node, folded into
the intent key as an explicit 5th component, re-minted only where
`Delivery::Proven` clears the draft — retries (incl. `approve()` replay of
stored params) keep their id, a new submission after a proven send gets a
fresh one; `_`-prefixed params are stripped at the operation-write dispatch
so the id never crosses the connector boundary.

## Missing piece

The two-submission sequence "submit, prove delivery, submit the SAME text
again" was never driven by any automated layer: the mcp-mock send_message
suite drove single sends and retries only, and the windowed input_box suite
never asserted submission identity. Missing example sequence, not a
generator/alphabet issue.

## Remedy

FIXED 2026-08-03 (compose-id lane). Gap-closing rungs, red-first: mock "a
second submission of identical text must produce its OWN queued intent"
(red: "the two submissions collapsed onto one key"), windowed "every
submission must carry its own `_compose_id`" (red: got None), unit
`intent_key_separates_submissions_but_not_retries`.
