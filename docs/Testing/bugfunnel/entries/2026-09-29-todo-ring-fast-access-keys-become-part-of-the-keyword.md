---
id: 2026-09-29-todo-ring-fast-access-keys-become-part-of-the-keyword
date: 2026-09-29
gap: COVERAGE
status: PARTIAL
summary: >-
  A `#+TODO:` ring spelled with org's fast-access keys or log markers
  (`NEXT(n) | DONE(d)`, `TODO(t!)`) declares the keyword `NEXT(n)` to Holon, so
  `* NEXT Plan` is read as title text, which org reads as the NEXT state.
---

## Bug
Found by the Inc 6 round-6 adversarial verifier (`lane-logs/inc6r6v-verify.md`,
D9, probes `zzv4`, `zzv6`, `zzv7`), and present on main `d4f426ffc965`. In a
file with `#+TODO: NEXT(n) | DONE(d)`, the headline `* NEXT Plan` is stored as
the title `NEXT Plan` with no state, and `* NEXT(n) Plan` is stored with the
state `NEXT(n)`. Org reads the first as the NEXT state and the second as title
text, so the meaning of each headline inverts between Holon and org.

## Root cause
`parse_todo_keywords_config` (`crates/holon-org-format/src/parser.rs`) took
each word of the `#+TODO:` line as a keyword verbatim. Org's keyword is the
text before `(`; the parenthesis holds the fast-access key and the log markers
(org manual, "Fast access to TODO states", "Tracking TODO state changes").

## Missing piece
No parser test and no generator spells a ring with `(key)` or `!`/`@` markers.
The keystone's `TodoKeywordSet` draws bare keywords only.

## Remedy
Inc 6 round 7, corrected in round 8 (below): the parser strips a ring word's
trailing parenthesis group, as org does. The document
keeps its authored `#+TODO:` line (`org_props::TODO_DECLARATION`), and
write-back renders that line verbatim while it still declares the document's
keywords, so the file keeps its fast-access keys and log markers. Tests:
`crates/holon-org-format/tests/todo_ring_spellings.rs` (red at base in
`lane-logs/inc6r7-red.log`) and
`a_fast_access_ring_declares_the_keyword_before_the_key` in
`crates/holon-integration-tests/tests/dense_patch_engine_exact.rs` (store,
file and the file's header spelling).

## Round 8
The round-7 verifier refuted the round-7 rule (`lane-logs/inc6r7v-verify.md`,
D11, D13): it cut every word at its first `(`, where org's
`org-remove-keyword-keys` strips `(.*)$` only (`FOO(bar` stays the keyword
`FOO(bar`); it read the ring by a raw line scan of the whole file, so a
`#+TODO:` line inside a preamble block became the ring and was written back as
a header; it read only the first ring line; and it ignored `#+TYP_TODO:`.
Round 8 reads the ring from the keyword elements of the text before the first
headline, takes every `#+TODO:`, `#+SEQ_TODO:` and `#+TYP_TODO:` line, strips a
trailing parenthesis group only, and makes a line's last word the done keyword
when it has no `|` (`TodoDeclaration` in `crates/holon-org-format/src/parser.rs`;
tests in `crates/holon-org-format/tests/todo_ring_spellings.rs`, red in
`lane-logs/inc6r8-red.log`).

Still open: org collects ring keywords from keyword elements ANYWHERE in the
file (`org--collect-keywords-1` in org.el searches the whole buffer). Holon
reads the text before the first headline only, as ruled for this round, so a
hand-authored `#+TODO:` keyword line below a headline declares a ring in org
and none in Holon. Holon itself never writes that shape: a `#+` line in block
text is written `,#+` (D230.a), and dense_patch refuses an unescaped one.

