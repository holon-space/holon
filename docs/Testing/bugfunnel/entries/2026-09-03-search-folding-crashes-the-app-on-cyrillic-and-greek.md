---
id: 2026-09-03-search-folding-crashes-the-app-on-cyrillic-and-greek
date: 2026-09-03
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Typing an ordinary Cyrillic or Greek phrase into quick-open aborts the whole
  process with a tokio-worker stack overflow.
---

## Bug

Found by the `dogfood-search` lane driving the live GPUI app (port 8730, real
vault copy, 2277 blocks) as the dogfood gate for the quick-open search fix.

Opening quick-open (cmd-K) and typing `программирование на русском языке`
kills the app outright:

    thread 'tokio-rt-worker' (74227928) has overflowed its stack
    fatal runtime error: stack overflow, aborting

The window disappears, the MCP port stops answering, and any unsaved editor
state goes with it. There is no error banner and no "Search failed" — the
process is simply gone. Reproduced twice from a clean launch: once with 27
distinct Latin accented capitals, once with the Russian phrase above.

The same predicate builder backs the `[[` link picker
(`search_link_candidates`), so the crash is reachable from ordinary note
editing too, not only from the search overlay.

## Root cause

`SearchMatch::folded_column` (`crates/holon/src/api/query_engine.rs:430-444`)
folds case by wrapping the stored column in one `replace()` per **distinct
cased non-ASCII letter in the query**, left-nested:

    replace(replace(replace(b.content, 'Ä', 'ä'), 'Ö', 'ö'), 'Ü', 'ü')

The nesting depth is therefore the query's distinct-accented-letter count, and
Turso's expression handling recurses once per level. Measured on the live app
by issuing hand-built nested expressions through `execute_raw_sql`, one depth
at a time from a clean launch: depths 1–14 return normally, **depth 15 aborts
the process**. `quick_open_search` embeds the folded expression twice per
statement (WHERE and ORDER BY) across two statements, so the real UI threshold
is at or below that.

The cost was reasoned about as "bounded by the query, never by the vault"
(the function's own doc comment), which is true for *width* and false for
*depth*. Latin-script queries stay far below the limit — a German sentence
reaches 3 — but scripts where every letter is non-ASCII hit it immediately:

| query | distinct cased non-ASCII letters |
|---|---|
| `Wörterbuch für Übersetzungen mit Änderungen` | 3 |
| `Đây là một câu tiếng Việt dài` | 6 |
| `Επεξεργασία κειμένου` (20 chars) | 15 |
| `программирование на русском языке` (33 chars) | 16 |

So a Greek or Russian user reaches the crash with a normal short phrase.

Evidence: `logs/dogfood-session-2026-09-03-search/app.log` (both aborts), and
the depth sweep in the lane report.

## Missing piece

The keystone's `Search` transition generates queries and asserts results, but
its adversarial arm draws `%`, `_`, quotes and case-perturbed ASCII — it never
draws a query with many DISTINCT non-ASCII letters, so the generated SQL never
nests deep enough. Two absences compound:

- no generator arm for multi-script / many-distinct-accented queries; and
- no bound on the generated predicate's nesting depth anywhere, so nothing
  fails before the process does.

A crashed process is also invisible to the harness as a *search* defect: the
run dies, so even an existing invariant could not report it.

## Remedy

FIXED. The predicate is a `GLOB` pattern instead of a folded column: every cased
letter of the query becomes the two-element character class `[<lower><upper>]`,
so the fold lives in the pattern and the SQL is one flat string whose *length*,
not depth, grows with the query. `GLOB` is case-sensitive, so the classes carry
ASCII folding too; it has no escape character, so its own metacharacters `*`,
`?` and `[` are spelled as one-element classes, and `%` / `_` — which `GLOB`
does not treat specially — stay literal by construction. Both call sites,
`quick_open_search` and `search_link_candidates`, build the predicate through
`SearchMatch::contained_in` / `prefix_of`, which emit the whole comparison, so a
call site cannot supply a bare column.

The stored-folded-column alternative (a shadow column maintained by the
projection) was not needed: the reference oracle folds per character, which a
character class expresses exactly, and a schema change would add a write-time
leg for a read-time concern.

Covering tests:

- `crates/holon-app/tests/search_deep_script_query_does_not_overflow.rs` — the
  48-distinct-letter Cyrillic + Greek query through both `quick_open_search` and
  `search_link_candidates`, on a 256 KiB-worker runtime so the depth bound is
  pinned independently of the platform's stack size. RED before the fix with
  `fatal runtime error: stack overflow, aborting`.
- `crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`,
  case `search-a-cyrillic-and-greek-query-does-not-crash-the-process`.
- The keystone `Search` transition's adversarial arm
  (`crates/holon-integration-tests/src/pbt/transitions/search.rs`) now draws the
  Russian and Greek phrases, the full two-alphabet query, and the `GLOB`
  metacharacters `*`, `?`, `[a-z]`.

Still open: NFC/NFD normalization, named in
`2026-09-03-search-does-not-fold-case-for-non-ascii-letters`.
