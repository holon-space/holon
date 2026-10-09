---
id: 2026-10-09-peer-json-text-nested-deeply-aborts-holon-in-fromjson
date: 2026-10-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A peer string of 2000 `[` (2 KB) ended the Holon process with a native stack overflow in
  any mapping that runs `fromjson` or `tonumber` on a response field.
---

## Bug
Found by the security verifier of the jaq mapping hardening (`lane-logs/jaq-verify.md`,
Round 2, R2-D1). `.p | fromjson` with input `{"p": "[[[…"}` (2000 `[`) aborted with exit 134
("has overflowed its stack") on a 2 MiB thread, the stack of a tokio worker. `tonumber` and
`toboolean` reach the same parser through jaq-json's `totype`. `try`/`?` does not help: the
abort happens before an error exists.

## Root cause
jaq-json's `fromjson` parses with a parser that recurses once per array or object level and
has no depth limit. The transport's serde depth limit (128) applies to the response document,
not to the text of a string inside it.

## Missing piece
No mapping test parsed peer-controlled JSON text, and no test ran a mapping on a stack the
size of the one it runs on in production.

## Remedy
`crates/holon-rows/src/jaq_library.rs` replaces jaq-json's `fromjson` native with one of the
same name and semantics that first scans the text (strings and `#` comments do not nest) and
refuses nesting past `MAX_FROMJSON_DEPTH` (128) with an error naming the limit. It stops at
the first parse error, because jaq's lexer resumes after an error wherever it stopped, which
can be inside a string the scan read as text. Tests in
`crates/holon-rows/tests/mapper_bounds.rs`:
`text_the_peer_nests_deeply_is_an_error_naming_the_limit_not_a_stack_overflow` (child process,
2 MiB thread, `fromjson`, `tonumber`, `[fromjson?]`) and
`fromjson_keeps_jaqs_semantics_up_to_the_nesting_limit`. Red log
`lane-logs/jaq-harden-r3-red.log` (SIGABRT); green `lane-logs/jaq-harden-r3-green1.log`.

Not covered here: a value a filter BUILDS can still be nested without limit
(`reduce range(100000) as $_ (null; [.])`, `def f: [f]; f`), and `tojson`, `tostring` and the
output serialization recurse over it. That is audit finding F2 and waits on ruling D-jaq-leg.
