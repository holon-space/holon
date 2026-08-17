---
id: 2026-08-03-shipped-chat-compose-box-can-never
date: 2026-08-03
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  The shipped chat compose box can NEVER deliver a message. Driving the real
  UI: type “ping one” → Send → the write is queued under `once_only:
  confirm_manually` (correct) → Approve → the provider rejects it verbatim:
  `MCP tool 'send_message' returned error: cannot answer: text is required and
  must be a string`. The app log shows exactly what was dispatched:
  `params={"message": String("ping one"), "_compose_id": String("e060f98f-…"),
  "id": String("5969a71e-26bf-4b92-b5a6-c00d2d93b8ac")}`. The real provider
  (`/Users/martin/Workspaces/ai/claude-code-history-mcp/src/server.rs:459-461`)
  reads `string_arg(args, "id")` and `string_arg(args, "text")`, and its
  published schema documents `id` as “live_session id from
  claude-history://live (a background SHORT id)”. So there are TWO independent
  mismatches: (i) the text parameter is named `message` in
  `docs/integrations/claude-history.yaml` (`input_box … text_param:
  "message"`) but `text` at the provider; (ii) the tool is declared `entity:
  session`, so it targets a `cc_session` transcript id (full UUID) where the
  provider needs the `cc_live_session` background short id (`5969a71e`) — the
  sidecar comment even notes the reachability property lives on
  `live_session`, and then targets `session` anyway. Verified end-to-end that
  nothing was delivered: the session's transcript still contains only its
  launch prompt after two send attempts. The UI's disclosure is honest
  throughout (persistent `not sent` strip, then `Outcome unknown — verify on
  remote`; no fabricated chat bubble, nothing recorded as delivered), so this
  is a wiring defect, not a truthfulness defect.
source_line: 1153
---

## Bug

(dogfood I6 gate, chat-input feature, throwaway vault on port 8710 against
an operator-created background Claude session) The shipped chat compose box
can NEVER deliver a message. Driving the real UI: type “ping one” → Send →
the write is queued under `once_only: confirm_manually` (correct) → Approve
→ the provider rejects it verbatim: `MCP tool 'send_message' returned error:
cannot answer: text is required and must be a string`. The app log shows
exactly what was dispatched: `params={"message": String("ping one"),
"_compose_id": String("e060f98f-…"), "id":
String("5969a71e-26bf-4b92-b5a6-c00d2d93b8ac")}`. The real provider
(`/Users/martin/Workspaces/ai/claude-code-history-mcp/src/server.rs:459-461`)
reads `string_arg(args, "id")` and `string_arg(args, "text")`, and its
published schema documents `id` as “live_session id from
claude-history://live (a background SHORT id)”. So there are TWO independent
mismatches: (i) the text parameter is named `message` in
`docs/integrations/claude-history.yaml` (`input_box … text_param:
"message"`) but `text` at the provider; (ii) the tool is declared `entity:
session`, so it targets a `cc_session` transcript id (full UUID) where the
provider needs the `cc_live_session` background short id (`5969a71e`) — the
sidecar comment even notes the reachability property lives on
`live_session`, and then targets `session` anyway. Verified end-to-end that
nothing was delivered: the session's transcript still contains only its
launch prompt after two send attempts. The UI's disclosure is honest
throughout (persistent `not sent` strip, then `Outcome unknown — verify on
remote`; no fabricated chat bubble, nothing recorded as delivered), so this
is a wiring defect, not a truthfulness defect.

## Missing piece

`crates/holon-mcp-mock` fixtures were authored FROM the sidecar, so mock and
sidecar agree on `message`/`session` and the whole automated stack is
self-consistent against a contract the real binary does not implement.
Missing piece = a contract check that reads the provider's advertised
`tools/list` schema and asserts every sidecar-declared tool's param NAMES
and target entity against it (the provider is a local binary and its schema
is machine-readable, so this is cheap and needs no network). Secondary
COVERAGE: no layer drives approve→dispatch against a real provider at all.

## Remedy

FIXED 2026-08-03 (send-contract lane). Red-first, with the MOCK re-authored
from the REAL binary rather than from the sidecar:
`crates/holon-mcp-mock/src/lib.rs` now publishes the provider's declared
`send_message` schema (`send_tool_schema`) and enforces it
(`check_send_contract`) — `text` and `id` required strings, `id` resolved
against the live listing by `live_session.id` or `job_id` only — rejecting
with the provider's verbatim text; the live fixture also drops its invented
keys (bg row is keyed by its JOB id, attached row by `pid-<pid>`, as
`live.rs` does). RED: all 7 existing `claude_history_send_message` tests
failed with the exact prod string `cannot answer: text is required and must
be a string` (log: `lane-sendcontract/red-mock.log`). THREE mapping seats
fixed, not two: `docs/integrations/claude-history.yaml` (`text_param:
"text"`; tool `entity: live_session`; compose box moved onto the
`live_session` profile binding `action: send_message(#{id: col("job_id")})`;
the `session` transcript profile is now read-only) and
`crates/holon-api/src/render_dsl.rs`, whose DSL alias hard-coded
`send_message` → `session.send_message`. Regression guards:
`the_message_must_ride_under_text_not_message`,
`a_send_addressed_by_transcript_id_reaches_no_session` (holon-mcp-mock),
`live_session_chat_view_composes_with_the_arguments_the_provider_declares`,
`session_chat_view_offers_no_compose_box` (holon-frontend). STILL OPEN,
tracked here: the generic contract check that diffs every sidecar-declared
tool against the provider's advertised `tools/list` schema — this fix pins
ONE tool by hand, so the class is closed only for `send_message`. Sibling
divergence found while doing it and NOT fixed: the mock's `answer_question`
takes `label` (string) where the real binary declares `answers` (array of
labels).
