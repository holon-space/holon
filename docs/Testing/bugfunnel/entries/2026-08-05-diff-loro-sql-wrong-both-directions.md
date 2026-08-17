---
id: 2026-08-05-diff-loro-sql-wrong-both-directions
date: 2026-08-05
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  diff_loro_sql wrong in BOTH directions: schemed doc_id minted
  block:block:<uuid> (sql=0); bare id left the Loro arm unscoped —
  resolve_by_doc_id ignores its argument (`_: &str`) and returns the global
  vault doc (1888 blocks, ~1840 false only_in_loro).
source_line: 773
---

## Bug

(agent vault work via live holon MCP) **diff_loro_sql wrong in BOTH
directions: schemed doc_id minted block:block:<uuid> (sql=0); bare id left
the Loro arm unscoped — resolve_by_doc_id ignores its argument (`_: &str`)
and returns the global vault doc (1888 blocks, ~1840 false only_in_loro).**
The Loro-vs-SQL convergence diagnostic was unusable exactly when needed.

## Root cause

diff_loro_sql wrong both directions — resolve_doc_uri minted
block:block:<uuid> on schemed input (closes the 2026-07-28 OPEN row) AND
LoroDocumentStore::resolve_by_doc_id ignores its argument (signature
literally `_: &str`) returning the global vault doc → 1888-block Loro arm,
~1840 false only_in_loro. Nothing ever invoked the tool in a test. FIXED:
idempotent from_raw at the boundary; ONE doc_subtree_ids walk serves both
arms (Page children + subtrees excluded on both, mirroring collect_subtree +
the Turso CTE); reject_parent_cycles errors; sql_row_node fails loud on
malformed tags; both-arms agreement property over schemed and bare ids)

## Missing piece

Nothing invokes the tool in any test; missing = membership property over
both arms (schemed ≡ bare; converged ⇒ equal counts; Page child excluded
both sides) + a live-MCP rung driving it with a list_loro_documents id
(published SCHEMED — hits mechanism (i) on the documented workflow).

## Remedy

FIXED 2026-08-05: idempotent from_raw boundary parse; ONE subtree walk for
both arms w/ Page-exclusion parity; reject_parent_cycles errors;
sql_row_node fails loud on malformed tags. Residual: no end-to-end test vs
real DB + real Loro doc (fixture shares parent_id normalization — a
field-only scheme divergence would pass the test); closes the 2026-07-28
resolve_doc_uri OPEN row.
