---
id: 2026-09-03-search-does-not-fold-case-for-non-ascii-letters
date: 2026-09-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Search folds case for ASCII but not for umlauts, so a German query typed with
  a capital Ü matches nothing while 28 blocks contain ü.
---

## Bug

Found by exploratory dogfooding (lane `dogfood-explore`) against a copy of
Martin's real, largely German vault.

The predicate `quick_open_search` builds is `content LIKE '%{query}%'`
(`crates/holon/src/api/query_engine.rs:104-122`). Measured against the live
engine on the ingested vault:

    content LIKE '%ü%'          -> 28
    content LIKE '%Ü%'          ->  0
    content LIKE '%zusammen%'   ->  1   (ASCII control)
    content LIKE '%ZUSAMMEN%'   ->  1   (ASCII control)

The ASCII control folds; the umlaut does not. A user searching `Übung`, `Möhren`
or `Gemüsebrühe` with the capital the word actually starts with gets nothing,
while the lower-case spelling of the same word matches. On a vault whose recipe
corpus is entirely German this is the common case, not an edge case.

There is also no unicode normalization on the path, so an NFC-composed query
cannot match NFD-decomposed content or the reverse.

## Root cause

`LIKE` in SQLite/Turso folds ASCII only, by design. The search path does no
folding or normalization of its own: the query string goes from the overlay into
the interpolated predicate with only `'` doubled
(`crates/holon/src/api/query_engine.rs:104`). Nothing in the chain lowercases
either side or normalizes to a canonical form.

## Missing piece

No search coverage of any kind (shared with
`2026-09-03-quick-open-search-returns-no-matches-for-every-query`), and no
generator that emits non-ASCII query strings. The keystone's alphabet has no
search transition to carry a unicode payload into.

## Remedy

Open. Fold both sides explicitly and normalize to one form before comparing;
pin it with a keystone case that searches a capitalized umlaut term and expects
the lower-case content to match. Note this bug is masked today by
`2026-09-03-quick-open-search-returns-no-matches-for-every-query` — search
returns nothing for any query — so the fix order is that entry first, this one
second, and the umlaut test only becomes meaningful after search works at all.

## Resolution (2026-09-03, lane `search-fix`)

FIXED for case; unicode NORMALIZATION remains open (see below).

`LIKE` folds ASCII and nothing else, so folding is now done on both sides.
`SearchMatch` simple-lowercases the query in Rust, and `folded_column` wraps the
stored column in one `replace()` per distinct cased non-ASCII letter the query
contains — so the cost is bounded by the query, never by the vault. Simple, not
full, folding: `ß` uppercases to `SS`, which a per-character `replace()` cannot
express, so it folds to itself (and `ẞ` with it, guarded by the round-trip check
in `simple_upper`) rather than half-folding into a match the pattern can never
reach.

NOT fixed and deliberately out of scope: NFC/NFD normalization. An NFD-decomposed
`u` + combining diaeresis still does not match an NFC `ü`. That is a separate
change (normalize at the ingest boundary, not in the predicate) and needs its own
entry when taken up.

## Covering tests

- Hand-authored keystone case `search-folds-case-for-non-ascii-letters`
  (`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`).
  Red-for-the-right-reason with the engine reverted:
  `quick_open_search("übung") missed block:searchumlaut in the In content
  section: its content "Übung" contains the query ... so nothing was truncated`.
- `crates/holon-app/tests/quick_open_search_at_vault_scale.rs` — `Übung`,
  `übung`, `ÜBUNG` and `üBuNg` must all find the one stored `Übung` block.
- The keystone `Search` oracle folds with the same simple-folding rule, and its
  generator case-perturbs drawn queries in Unicode (not ASCII), so a `é` in
  generated content also arrives as `É`.
