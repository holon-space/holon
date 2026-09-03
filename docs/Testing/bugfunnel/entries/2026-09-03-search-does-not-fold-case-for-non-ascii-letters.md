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

FIXED for case, including the many-to-one folds; unicode NORMALIZATION remains
open (see below).

`LIKE` folds ASCII and nothing else, so the predicate is now a `GLOB` pattern
built in Rust: every cased query character carries a character class holding its
whole SIMPLE-lowercase equivalence set. `FOLD_CLASSES`
(`crates/holon/src/api/query_engine.rs`) is a table built by scanning every
Unicode scalar and grouping by simple lowercase — no character is hand-listed,
so no member of a class can be forgotten. The pattern's SIZE grows with the
query length, its nesting depth never does.

Simple, not full, folding — the same rule the keystone oracle uses. A character
whose case mapping is multi-character maps to itself, so `ß` and `ss` are NOT
equivalent: `"ss"` compiles to `[Ss][Ss]`, and all-caps ASCII German `STRASSE`
stays unreachable from `straße` (`STRAẞE` is reachable, via the ß/ẞ class).
Unicode FULL folding would also put `ſ` (U+017F) in `s`'s class; simple folding
does not, so stored `ſ` is only found by `ſ`.

The first attempt at this fix emitted a two-element `[lower upper]` class per
character. That expresses a one-to-one fold and nothing else, so every character
sharing a fold with a THIRD spelling stayed unreachable: `ẞ` from `ß`, the
`ǅ`/`ǆ`/`Ǆ` digraph family from each other, and the Kelvin/Ohm/Angstrom signs
from their letter spellings. Adversarial verification (`search-fix-verify.md`)
produced seven end-to-end oracle divergences on exactly those characters — the
German pair being the user-facing one, since all-caps German writes `STRAẞE`.
Widening each class to the whole fold set closes all seven.

A query whose folded pattern would exceed Turso's GLOB length ceiling
(`MAX_GLOB_PATTERN_BYTES`) is refused with a typed `SearchQueryTooLong` error
naming the limit, rather than reaching the engine.

NOT fixed and deliberately out of scope: NFC/NFD normalization. An NFD-decomposed
`u` + combining diaeresis still does not match an NFC `ü`. That is a separate
change (normalize at the ingest boundary, not in the predicate) and needs its own
entry when taken up.

## Covering tests

- Hand-authored keystone case
  `search-many-to-one-case-folds-reach-every-spelling`
  (`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`) —
  content `Grüße Straße`, `STRAẞE capital`, `ǅungla titled`; queries `GRÜẞE`,
  `straße`, `ẞ`, `ß`, `ǅ`, `ǆ`, `Ǆ`, i.e. every divergence the verifier found.
  Green: `lane-logs/r4-handauthored-1789100000.log`
  (`PASSED case "search-many-to-one-case-folds-reach-every-spelling"`,
  `test result: ok. 9 passed; 0 failed`).
- Hand-authored keystone case `search-folds-case-for-non-ascii-letters` (same
  file). Red-for-the-right-reason with the engine reverted:
  `quick_open_search("übung") missed block:searchumlaut in the In content
  section: its content "Übung" contains the query ... so nothing was truncated`.
- `crates/holon/src/api/query_engine.rs`, `mod fold_class_tests` —
  `glob_class_is_the_oracles_whole_equivalence_class_across_the_bmp` sweeps the
  BMP (over 2000 cased characters) and asserts the emitted class equals the
  oracle's fold class for each; `the_many_to_one_folds_reach_every_spelling`
  pins ß/ẞ, the `ǅ` family and the unit signs by name;
  `an_over_long_query_is_refused_at_the_exact_threshold` pins the
  `SearchQueryTooLong` refusal at the `MAX_GLOB_PATTERN_BYTES` boundary.
- `crates/holon-app/tests/quick_open_search_at_vault_scale.rs` — `Übung`,
  `übung`, `ÜBUNG` and `üBuNg` must all find the one stored `Übung` block.
- The keystone `Search` oracle folds with the same simple-folding rule, and its
  generator case-perturbs drawn queries in Unicode (not ASCII), so a `é` in
  generated content also arrives as `É`. `ADVERSARIAL_QUERIES` carries the
  many-to-one characters, but the generated content alphabet is `[a-z]`, so
  those queries are vacuous there — the teeth are the hand-authored case above.

## Dogfood re-run 2026-09-03

Re-driven live on the real vault copy (lane `dogfood-search`). This vault holds
no `Übung`, so the check used its own content: the query `NÄCHSTEN` finds the
block whose stored text reads `... an den nächsten`. Folding works in the
direction the entry reports as broken. Screenshot `10-search-NAECHSTEN.png`.

The nested-`replace()` folding this run exercised is what
`2026-09-03-search-folding-crashes-the-app-on-cyrillic-and-greek` (FIXED)
reports; the `GLOB` fold-class pattern above replaces it.
