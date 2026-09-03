---
id: 2026-09-03-search-treats-like-metacharacters-as-wildcards
date: 2026-09-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The search query is interpolated into a LIKE pattern without escaping % or _,
  so those characters act as wildcards and every hit is a false positive.
---

## Bug

Found by exploratory dogfooding (lane `dogfood-explore`) against a copy of
Martin's real vault (2257 blocks, 128 pages).

`quick_open_search` escapes only the single quote before interpolating the query
into `content LIKE '%{query}%'` (`crates/holon/src/api/query_engine.rs:104`).
`%` and `_` therefore reach the engine as pattern metacharacters. Measured live:

    query "%"     page branch    -> 132 hits;   pages actually containing "%" -> 0
    query "o_e"   content branch -> 903 hits;   blocks containing "o_e"       -> 0

Every hit in both cases is a false positive: a single `%` matches the entire
page set, and `o_e` matches any three characters in that shape. `_` is not
exotic in this vault — underscored identifiers (`_drawer_order`,
`_change_origin`) are a documented convention — so a user searching for one gets
a result set with no relationship to what they typed.

The same unescaped interpolation is used by `search_link_candidates`
(`crates/holon/src/api/query_engine.rs:80-97`), which backs the `[[` link popup,
so the link picker has the identical defect.

## Root cause

String interpolation into a `LIKE` pattern with an incomplete escape set: the
quote is handled, the two pattern metacharacters are not, and no `ESCAPE` clause
is supplied. This is the "parse, don't validate" failure the project warns
about — a raw user string is spliced into a predicate instead of being converted
into a typed, escaped pattern at the boundary.

## Missing piece

No search coverage at all (shared with
`2026-09-03-quick-open-search-returns-no-matches-for-every-query`), so no
generator ever emits a query containing `%` or `_`, and no invariant compares a
search's result set against a literal-substring oracle.

## Remedy

Open. Escape `%`, `_` and the escape character itself and pass an explicit
`ESCAPE`, or move the match to a function that takes a literal. Pin it with a
keystone case whose query contains each metacharacter and whose oracle is
`instr(content, query) > 0`. As with the umlaut entry, this is masked today by
search returning nothing for every query, so it is fixed after that one.
