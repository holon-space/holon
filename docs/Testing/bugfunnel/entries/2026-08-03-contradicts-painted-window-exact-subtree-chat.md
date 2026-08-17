---
id: 2026-08-03-contradicts-painted-window-exact-subtree-chat
date: 2026-08-03
gap: PERCEPTION
secondary: ORACLE
status: OPEN
summary: >-
  `describe_ui` contradicts the painted window on the exact subtree the chat
  feature lives in, so the I6 gate cannot be run headlessly. After a click
  that visibly expands a session row — the window shows the chevron as `▼`,
  the message area, the compose box with placeholder `Message this session`
  and a `Send` button — `describe_ui` on the same panel still prints
  `expand_toggle(▶ cc-session:…) content=UNEVALUATED` with `content thunk not
  forced: the expand_toggle gate is closed`. The tool's own geometry block
  DISAGREES with its own widget tree in the same response: it records
  `expand_toggle cc-session:… x=376.0 y=437.0 w=848.0 h=154.0 visible`, i.e.
  the expanded height, while the tree says the gate is closed. No `input_box`
  or button node appears anywhere in the snapshot. Companion defect on the
  sidebar `live_query`: the unevaluated marker reads `rows not evaluated: the
  node carries no query/query_lang/render_expr, so it cannot describe its own
  result`, yet the very same JSON node carries `"query": "SELECT
  provider_name, updated_at FROM sync_states …"` and `"query_lang":
  "holon_sql"` — the stated reason is false; only `render_expr` is missing.
source_line: 1155
---

## Bug

(dogfood I6 gate, chat-input feature, same session) `describe_ui`
contradicts the painted window on the exact subtree the chat feature lives
in, so the I6 gate cannot be run headlessly. After a click that visibly
expands a session row — the window shows the chevron as `▼`, the message
area, the compose box with placeholder `Message this session` and a `Send`
button — `describe_ui` on the same panel still prints `expand_toggle(▶
cc-session:…) content=UNEVALUATED` with `content thunk not forced: the
expand_toggle gate is closed`. The tool's own geometry block DISAGREES with
its own widget tree in the same response: it records `expand_toggle
cc-session:… x=376.0 y=437.0 w=848.0 h=154.0 visible`, i.e. the expanded
height, while the tree says the gate is closed. No `input_box` or button
node appears anywhere in the snapshot. Companion defect on the sidebar
`live_query`: the unevaluated marker reads `rows not evaluated: the node
carries no query/query_lang/render_expr, so it cannot describe its own
result`, yet the very same JSON node carries `"query": "SELECT
provider_name, updated_at FROM sync_states …"` and `"query_lang":
"holon_sql"` — the stated reason is false; only `render_expr` is missing.

## Missing piece

Distinct from the existing `with_data_rows(vec![])` row: there the tool
invented an empty result, here it reports a live, painted, EXPANDED subtree
as closed and unevaluated, and its two halves (widget tree vs geometry)
disagree inside one response. Nothing cross-checks the snapshot against the
measured geometry, which is a cheap and fully mechanical oracle: any node
with recorded visible bounds taller than its collapsed header must not be
reported as gated-closed. Missing piece = that self-consistency assertion,
plus an unevaluated reason string derived from what is ACTUALLY absent.

## Remedy

OPEN 2026-08-03 — diagnosis only. Consequence for process: every I6 verdict
in this run rests on screenshots, because the sanctioned headless oracle
would have reported the working chat view as unrendered.
