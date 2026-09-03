---
id: 2026-09-03-quick-open-search-returns-no-matches-for-every-query
date: 2026-09-03
gap: COVERAGE
secondary: null
status: FIXED
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

## Resolution (2026-09-03, lane `search-fix`)

FIXED. The mechanism was never in the overlay: it was the Pages branch's `JOIN`
against the `block` MATVIEW. Measured on the 2257-block / 129-page fixture, the
two spellings of the same predicate:

    ... FROM block b JOIN block_tags bt ON bt.block_id = b.id
        WHERE bt.tag = 'Page' AND b.content LIKE '%Compass%'   -> 10.74 s
    ... FROM block b WHERE b.id IN (SELECT block_id FROM block_tags
        WHERE tag = 'Page') AND b.content LIKE '%Compass%'     ->  0.053 s

That is the whole "silent" part of the escape. `run_search` drops any response a
newer keystroke has overtaken (`s.generation == generation`), so at 5-10 s per
call every response lost the race and the overlay kept rendering its empty
state — no error was swallowed, no error ever existed. The predicate now uses
the `IN` subquery; end-to-end `quick_open_search` on the same fixture is 106 ms
for a selective query.

The fail-loud leg was checked and reinforced rather than added: a query error
already becomes `s.error` and is rendered instead of the empty state, and the
two places that could have discarded a result silently (`tx.send` on a dropped
receiver, `update_window` on a closed window) now log an ERROR instead of
`let _ =`.

## Covering tests

- `crates/holon-integration-tests/src/pbt/transitions/search.rs` — keystone
  `Search` transition: hits are compared against the reference model's own
  substring match, per section, for soundness and (untruncated) completeness.
- `crates/holon-app/tests/quick_open_search_at_vault_scale.rs` — deterministic
  pin at real-vault scale, incl. a per-keystroke latency budget that fails if a
  search becomes slow enough to lose the newest-response race again.
