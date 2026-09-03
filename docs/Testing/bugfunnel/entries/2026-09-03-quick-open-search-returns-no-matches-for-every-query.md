---
id: 2026-09-03-quick-open-search-returns-no-matches-for-every-query
date: 2026-09-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The cmd-k quick-open overlay reports "No matches" for every query on a
  2257-block vault, including terms whose backing SQL returns rows.
---

## Bug

Found by exploratory dogfooding (lane `dogfood-explore`) against a copy of
Martin's real vault: 131 org documents, 2257 blocks, 128 pages, MCP port 8720.

Opening the quick-open overlay and typing `Suppe` renders `No matches for
"Suppe"` while the breadcrumb of the very same window reads `Resources > Rezepte
> Linsensuppe`. Typing `Compass` renders `No matches for "Compass"` while the
left sidebar shows a `Compass` page and the main panel is full of the word.

The same predicate run directly against the engine returns rows:

    SELECT count(*) FROM block WHERE content LIKE '%Suppe%'   -> 2
    -- page branch, same shape as quick_open_search:
    SELECT b.id FROM block b JOIN block_tags bt ON bt.block_id = b.id
      WHERE bt.tag = 'Page' AND b.content LIKE '%Suppe%'      -> block:16db6fd4-… (Linsensuppe)

So the data is present and the predicate matches; the overlay still shows the
empty state. No WARN, no ERROR, and no `quick_open` line of any kind reaches the
app log — the failure is completely silent, which is itself a violation of the
project's fail-loud rule: the user is told "no matches" when the truthful answer
is "the search did not work".

Evidence: `logs/dogfood-session-2026-09-03/07-search-suppe.png`,
`08-search-compass.png`.

## Root cause

Not yet isolated — this entry records the escape, not the mechanism. What is
established: the SQL the search builds
(`crates/holon/src/api/query_engine.rs:99-130`) returns rows for these queries
when executed directly, so the defect is between `QueryEngine::quick_open_search`
and the overlay's result rendering (`frontends/gpui/src/search_ui.rs:172-240`),
not in the predicate. The silence rules out a surfaced error and points at a
swallowed result or a state that never reaches the overlay's render.

## Missing piece

There is no automated coverage of search anywhere in the tree:
`quick_open_search` has no test reference outside its own definition and the
`holon-api` default that bails; the keystone catalog has no search transition;
`ReferenceState::search_link_candidates` explicitly bails
(`crates/holon-integration-tests/src/pbt/reference_state.rs:3368`); and the bug
funnel had zero entries mentioning search before this one. A feature with no
generator and no oracle can regress to total non-function without any gate
noticing.

Compounding it, the overlay is invisible to the dogfood surface: the title-bar
toolbar and the search overlay do not appear in `describe_ui` at all (see
`2026-09-03-titlebar-toolbar-is-invisible-to-describe-ui`), so even an agent
driving the live app cannot assert on search state without screenshots.

## Remedy

Open. The fix must start with the covering test, per the feature contract: add a
search transition to the keystone catalog plus an invariant that a query whose
backing predicate matches at least one block must not render the empty state.
That test has to go red on this build before the overlay is touched.
