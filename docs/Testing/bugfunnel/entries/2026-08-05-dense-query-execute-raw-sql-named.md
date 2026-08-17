---
id: 2026-08-05-dense-query-execute-raw-sql-named
date: 2026-08-05
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  dense_query/execute_raw_sql named params silently return 0 rows:
  bind_parameters recognized only $name — :name/@name placeholders copied
  verbatim into the statement, bound NULL, matched nothing, succeeded.
source_line: 772
---

## Bug

(agent vault work via live holon MCP) **dense_query/execute_raw_sql named
params silently return 0 rows: bind_parameters recognized only $name —
:name/@name placeholders copied verbatim into the statement, bound NULL,
matched nothing, succeeded.** App-wide binding seam, not a dense_query
quirk.

## Root cause

secondary ORACLE: dense_query named params silently returned 0 rows —
bind_parameters (turso.rs:960) recognized ONLY $name; :name/@name copied
verbatim, bound NULL, succeeded empty, on EVERY app SQL surface
(execute_raw_sql shares the seam). No test used any sigil but $; the only
catching oracle is param-form ≡ literal-form parity, which nothing
expressed. FIXED: shared literal/comment-aware rewrite_named_params ($ : @,
quote/comment spans skipped — load-bearing, schemed ids put colons in most
literals; mutation-proven against 2 prod-schema tests), fail-loud on unbound
placeholders, duplicate scanner unified; e2e parity test vs real Turso
0-vs-5 red captured)

## Missing piece

No test used any sigil but $; even a covering success-assert would stay
green — the only catching oracle is param-form ≡ literal-form parity.

## Remedy

FIXED 2026-08-05: shared rewrite_named_params (literal/comment-aware, all
three SQLite sigils; quote-skip mutation-proven against prod-schema tests),
fail-loud unbound-placeholder errors naming the param, e2e parity vs real
Turso (red 0-vs-5). Residual: inline_parameters leave-verbatim fallback =
#40; unterminated-literal swallows later placeholders (unpreparable SQL
anyway); x::text cast reads as :text placeholder (fail-loud either way).
