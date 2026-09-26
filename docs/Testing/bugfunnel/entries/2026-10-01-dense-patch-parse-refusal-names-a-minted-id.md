---
id: 2026-10-01-dense-patch-parse-refusal-names-a-minted-id
date: 2026-10-01
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  When the org parser refused a row of an edited dense text (a typed drawer
  key such as `:Task_State:`, a bad `:WIDGET_ONLY:` value, a lower-case
  priority cookie), dense_patch named a uuid the parser had minted, not the
  `{#alias}` row the agent wrote.
---

## Bug
Found by the Inc 6 round-3 verifier (`lane-logs/inc6rb3v-verify.md`, defect A),
lane decision Inc 6. Edit `:owner: me` to `:owner: me\n:Task_State: x`: the
refusal read `block 726e65c2-...: drawer key :Task_State: is refused`.

## Root cause
The per-headline causes of the org parser (`crates/holon-org-format/src/parser.rs`)
named `block {id}`, the id the parser mints for a headline with no `:ID:`.
dense_patch passed the parse error through, so the agent got an id that
exists nowhere in its text.

## Missing piece
Oracle: the judge's "the refusal names a row" check accepted a quoted `{#0}`
anywhere in the message. Coverage: the generator drew no typed drawer key, so
no case reached a parse-level refusal.

## Remedy
The parser wraps each headline's refusal in `HeadlineRefused` (title, authored
`:ID:`, cause); the causes no longer name a minted id. `refused_by_row`
(`crates/holon-org-format/src/dense.rs`) turns it into `row {#a}: cause` or
`new row "title": cause`. Pinned by `every_parse_refusal_names_its_row`
(`crates/holon-org-format/tests/dense_parse_refusal_names_the_row.rs`, all 12
row-level parse refusal sites) and by the engine judge, now strict, over
`a_text_org_writes_otherwise_is_refused_before_any_write` and the generator
(which draws typed drawer keys).
