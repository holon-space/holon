---
id: 2026-08-03-chat-view-renders-messages-ever-transcript
date: 2026-08-03
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  The chat view renders NO messages, ever — the transcript area shows a red
  `Query error` instead of bubbles: `Failed to create materialized view
  watch_view_9ef7f36587fc903c: CREATE MATERIALIZED VIEW IF NOT EXISTS … AS
  SELECT uuid, role, content, timestamp AS ts FROM cc_message_fdw WHERE
  session_id = 'cc-session:5969a71e-26bf-4b92-b5a6-c00d2d93b8ac' AND role IS
  NOT NULL`. Two independent defects in one line. (i) The shipped `cc_session`
  profile in `docs/integrations/claude-history.yaml` backs its bubbles with a
  `live_query`, and the watch path materialises every live query as a matview
  — but Turso will not create one over a FOREIGN table, so the query fails
  before it can return a row. (ii) Even if it created, `$context_id` is
  substituted as the SCHEME-PREFIXED entity id (`cc-session:…`) while
  `cc_message.session_id` stores RAW ids — the sidecar's own fan-out SQL says
  so, using `substr(cc_session.id, 12)` to strip the scheme before joining. So
  the predicate would match zero rows on its own. Confirmed independently:
  `SELECT count(*) FROM cc_message` is 0 after opening the view. The failure
  IS disclosed loudly and legibly in the UI (correct per the error
  philosophy), which is why this was findable in one screenshot.
source_line: 1154
---

## Bug

(dogfood I6 gate, chat-input feature, same session) The chat view renders NO
messages, ever — the transcript area shows a red `Query error` instead of
bubbles: `Failed to create materialized view watch_view_9ef7f36587fc903c:
CREATE MATERIALIZED VIEW IF NOT EXISTS … AS SELECT uuid, role, content,
timestamp AS ts FROM cc_message_fdw WHERE session_id =
'cc-session:5969a71e-26bf-4b92-b5a6-c00d2d93b8ac' AND role IS NOT NULL`. Two
independent defects in one line. (i) The shipped `cc_session` profile in
`docs/integrations/claude-history.yaml` backs its bubbles with a
`live_query`, and the watch path materialises every live query as a matview
— but Turso will not create one over a FOREIGN table, so the query fails
before it can return a row. (ii) Even if it created, `$context_id` is
substituted as the SCHEME-PREFIXED entity id (`cc-session:…`) while
`cc_message.session_id` stores RAW ids — the sidecar's own fan-out SQL says
so, using `substr(cc_session.id, 12)` to strip the scheme before joining. So
the predicate would match zero rows on its own. Confirmed independently:
`SELECT count(*) FROM cc_message` is 0 after opening the view. The failure
IS disclosed loudly and legibly in the UI (correct per the error
philosophy), which is why this was findable in one screenshot.

## Root cause

dogfood I6, chat-input gate — the chat view can never show a message, for
two independent reasons, both in the shipped `live_query` SQL: (i) it reads
`cc_message_fdw`, the FOREIGN table, while
`MatviewManager::prime_fdw_caches` keys on the CACHE name (`cc_message`) —
so the cache is never primed AND the matview sits on a source that emits no
deltas, making it a one-shot snapshot rather than a stream; (ii)
`$context_id` substitutes the SCHEME-PREFIXED entity id against
`cc_message.session_id`, which stores raw ids; `chat_view_render.rs` stubs
`watch_query` with an empty-but-live stream, so no layer ever ran the
shipped profile against a real Turso) — BOTH CAUSES FIXED AT THE SQL LEVEL
2026-08-04: profile routed to `cc_message`/`cc_agent_message` and joined on
the new `$context_local_id` (bound once in
`BackendEngine::bind_context_params` off the parsed `EntityUri`, so the two
CONTEXT-param joins no longer re-derive the scheme strip inline; the three
`enumerate_from.where` clauses at `claude-history.yaml:284,:433,:505` still
use `substr(cc_*.id, N)` and must — they run per candidate row of a table
scan, where the schemed id is a COLUMN and no query context exists for a
param to come from). Covered by
`crates/holon-integration-tests/tests/chat_view_message_join.rs`, which
executes the SHIPPED sql against mirror-shaped fixtures.
Red-for-the-right-reason for (ii) was `left: [] right: ["FIRST", "SECOND"]`.
Confirmed against the live vault: the schemed join matches 0 rows where the
local join matches 11/7/806. Correction to the original entry: Turso does
NOT refuse `CREATE MATERIALIZED VIEW` over a foreign table — the durable
evidence is the fork's own test corpus (`test_fdw_matview_holon_shape.rs`
rungs 1-5 green, and the pre-existing `test_refresh_matview_on_fdw`), per
docs/Plans/turso-fdw-ivm-handoff-2026-08-04.md; separately, a live
cache-table matview was observed being incrementally maintained at 3779
rows, corroborating the routing fix. `cc_message` is also not empty (4603
rows). The recorded DDL error's real cause was never captured —
`.with_context` put it in a source that every consumer drops when printing
with `{}`; `matview_manager.rs` now spells it into the message so the next
occurrence records it. OPEN RESIDUALS: (a) end-to-end bubble painting is
UNVERIFIED — needs a rebuilt binary against the real sidecar, so the dogfood
gate has NOT been re-run; (b) `cc_agent_message` does not exist in the live
DB (`sqlite_master` lists no such table), so the agent chat view will hit
`no such table` until that vtable registers — not a regression (it failed
identically under the old `_fdw` name) but it blocks the agent half of an I6
re-dogfood; (c) the only guard for cause (i) is the `_fdw` string sweep,
which reads `profile_variants[0]` and the FIRST `live_query` per render —
complete today (3 live_query sites, 1 variant each) but silently incomplete
if a second variant or a second live_query appears; candidate hardening for
a follow-up; (d) `stream_check` exercises a matview over a plain btree table
with no FDW present, so it proves streaming works, not that it would have
caught the defect.

## Missing piece

`crates/holon-frontend/tests/chat_view_render.rs` deliberately stubs
`watch_query` with an empty-but-live stream so the nesting under test
survives — which means the ONE test that reads the shipped profile can never
see that the profile's query is unrunnable on the real backend. No layer
runs a sidecar profile's `live_query` against a real Turso with a real fdw.
Missing piece = an integration test that boots the real backend, opens a
shipped entity profile, and asserts the live_query produces ROWS (not merely
that a node exists) — the same `rendered items == query rows` oracle already
proposed for the nested-live_query row, extended to sidecar-declared
profiles.

## Remedy

OPEN 2026-08-03 — diagnosis only, BLOCKS shipping the chat view. Fix
direction needs a ruling: either the watch path must serve fdw-backed
queries without a matview (poll/refetch), or the profile must read the
`cc_message` CACHE table (which the vtable write-through fills) instead of
`cc_message_fdw`. The `$context_id` prefix mismatch must be fixed either
way.
