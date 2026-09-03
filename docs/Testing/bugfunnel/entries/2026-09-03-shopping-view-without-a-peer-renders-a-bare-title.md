---
id: 2026-09-03-shopping-view-without-a-peer-renders-a-bare-title
date: 2026-09-03
gap: ENVIRONMENT
secondary: PERCEPTION
status: OPEN
summary: >-
  With no shopping peer configured the view renders one bullet reading "Shopping
  list" and nothing else — no header, no empty state, no statement that no peer
  is connected.
---

## Bug

Found by exploratory dogfooding (lane `dogfood-explore`), driving the live app
with no shopping peer configured — the state every user is in before pairing.

Navigating to `block:shopping-view` renders a single row, the bullet and the
text `Shopping list`. The rest of the panel is blank. `describe_ui` of the main
panel confirms it: one `rendered_text "Shopping list"` and a `drop_zone`,
nothing more.

The view's own render source is not blank. `block:shopping-view::render::0`
holds

    column(text("To buy, by aisle", #{muted: true}),
           live_query(#{sql: "SELECT id, name, cat, count FROM shopping_item
                             WHERE deleted_at IS NULL AND checked = 0
                             ORDER BY cat, name", …}))

so the intended surface is a muted header over a list. Neither the header nor
the list renders. `shopping_item` exists as a materialized view and holds 0
rows, so the query is valid and empty.

A user opening this view sees a title and blank space, with nothing
distinguishing "no peer configured" from "your list is empty" from "the view is
broken". Screenshot: `logs/dogfood-session-2026-09-03/03-shopping-view.png`.

## Root cause

Two layers, only the second of which is a design gap. First, the render source
block is not producing its widget tree at all in this state — the header, which
is a literal string with no dependency on the query, should render regardless
and does not. Second, even had it rendered, the design has no degraded state:
`shopping_sync` is not registered when no peer is configured
(`crates/holon-app/src/mcp_integrations.rs:790-808`) and the only loud refusal
(`crates/holon-app/src/shopping_operations.rs:66-73`) sits on the sync path,
which this view never touches. The disconnected case therefore has no disclosure
anywhere in the UI.

## Missing piece

The keystone's environment has no peer-less integration surface — there is no
fixture in which a view's backing integration is absent — so the state cannot be
reached, let alone asserted on. No invariant requires that a view whose data
source is unavailable says so.

## Remedy

Open. The view needs an explicit disconnected state naming the missing peer, and
the render-source failure needs isolating first (a literal text node that does
not paint is the more alarming half). Test parity: a fixture with a declared but
unconfigured integration, plus an invariant that such a view renders a
disclosure rather than an empty body.
