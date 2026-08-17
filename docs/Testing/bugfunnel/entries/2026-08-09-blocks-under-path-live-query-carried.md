---
id: 2026-08-09-blocks-under-path-live-query-carried
date: 2026-08-09
gap: PERCEPTION
secondary: COVERAGE
status: FIXED
summary: >-
  A "blocks under this path" live query carried an ambiguous `Option<String>`
  path prefix whose `None` conflated "no filter wanted" with "path
  unresolved", and both silently produced WRONG rows.
source_line: 749
---

## Bug

(task #41 (+#45) lane, found by code exploration root-causing why the
nested-page chevron class (#27) recurred for six rounds; no test asserted
the non-silent outcome) **A "blocks under this path" live query carried an
ambiguous `Option<String>` path prefix whose `None` conflated "no filter
wanted" with "path unresolved", and both silently produced WRONG rows.**
`bind_context_params` (`crates/holon/src/api/backend_engine.rs`) bound a
`None` prefix as `Value::String("__NO_PATH__/")`, so `from descendants`
compiled to `path LIKE '__NO_PATH__/' \ | \ | '%'` — ZERO rows, a populated
nested page rendered empty with no error. Twin (#45):
`BlockDomain::lookup_block_path` returned `Ok("/{block_id}")` for a block
absent from `block_with_path` (a fabricated path no sibling shares → silent
mis-scope), and `HolonService::build_context` swallowed the same lookup
`Err` into the identical `/{id}`. All modes silent: no ERROR/WARN, invisible
to every invariant — why the chevron hunt needed six rounds.

## Root cause

task #41 (+#45) lane, found by code exploration root-causing why the
nested-page chevron class (#27) recurred at each new caller for six rounds —
no test asserted the non-silent outcome: **a "blocks under this path" live
query carried an AMBIGUOUS `Option<String>` path prefix whose `None`
conflated two opposite intents — "no filter wanted" and "path unresolved" —
and both silently produced WRONG rows.** At the bind seam
(`backend_engine.rs` `bind_context_params`) a `None` prefix bound the
sentinel `Value::String("__NO_PATH__/")`, so `from descendants` compiled to
`path LIKE '__NO_PATH__/' || '%'` and matched ZERO rows — a populated nested
page rendered empty with no error. #45 is the twin:
`BlockDomain::lookup_block_path` returned `Ok(format!("/{block_id}"))` for a
block absent from `block_with_path`, fabricating a path no other block
shares, so descendants silently mis-scoped; `HolonService::build_context`
then swallowed that same lookup `Err` into the identical `/{id}`
fabrication. PERCEPTION: every failure mode was a silent
zero-row/wrong-scope result — no ERROR, no WARN, nothing an invariant or a
human could see — which is why the chevron hunt took six rounds and why #43
(windowed subscriber) had to precede this. RULING C (Martin): split the
`Option` into a typed `PathContext { Unfiltered, Under(prefix) }` so the
ambiguous `None` is unrepresentable — `Unfiltered` binds an empty prefix
(`text.starts_with ''` matches ALL rows), `Under` binds the resolved
subtree, and "unresolved" is never a variant: it is an `Err` at resolution,
surfaced as a visible degraded banner (gpui/ply `resolve_block_path` now
`Err`-not-`for_block`, MCP `build_context` propagates). Secondary COVERAGE:
no headless rung ever drove a root/None-context `from descendants` or a
missing-block `lookup_block_path` and asserted Err-or-unfiltered rather than
silent-empty — the enforcement the six-round hunt lacked. FIXED in-lane
2026-08-09: sentinel DELETED (`prefix_literal()` empty for `Unfiltered`);
`lookup_block_path` fails loud; round-6 gpui/ply `for_block` sentinel
fallbacks removed. Red-first `crates/holon/src/api/backend_engine.rs` tests
(`root_context_binds_unfiltered_path_prefix_not_sentinel` red
`Some("__NO_PATH__/")` vs `""`;
`descendants_under_root_binds_no_sentinel_predicate` red on `path LIKE
'__NO_PATH__/' || '%'`;
`lookup_block_path_errs_on_missing_block_not_fabricated` red
`Ok("/block:does-not-exist-9f3a")`), all green after; logs
`lane-logs/red-41-45.log`, `lane-logs/red-descendants.log`. See the Ledger
row for the caller inventory and the redundant round-6 workarounds removed.)

## Missing piece

PERCEPTION: every failure was a silent zero-row / wrong-scope result with no
instrument to record it, so no assertion of any class could fire; #43
(windowed subscriber) had to land first. COVERAGE: no headless rung drove a
root/None-context `from descendants` or a missing-block `lookup_block_path`
asserting Err-or-unfiltered rather than silent-empty. Missing piece = a
typed boundary that makes the ambiguous `None` unrepresentable, plus a rung
that reds on silent-empty.

## Remedy

**FIXED in-lane 2026-08-09 (RULING C, Martin).** The `Option<String>` prefix
becomes a typed `PathContext { Unfiltered, Under(String) }`
(`crates/holon-api/src/query_context.rs`): `Unfiltered` binds an EMPTY
prefix (`text.starts_with ''` matches every row), `Under` binds the resolved
subtree, and "unresolved" is NOT a variant — it is an `Err` at resolution,
surfaced as a visible degraded banner. Sentinel DELETED. `lookup_block_path`
now `bail!`s (mirroring the adjacent `breadcrumb_trail` fail-loud);
`build_context` returns `Result<Option<QueryContext>>` and propagates.
Caller inventory: `render_entity`/`build_context` already resolved a real
path (now Err-not-fabricate); gpui + ply `live_query::resolve_block_path`
return `Result<String,_>` and paint a banner on failure — the round-6 (#27)
`None => QueryContext::for_block` sentinel fallbacks are REMOVED as
redundant; MCP `describe_ui_expand` already propagated the Err (comment
corrected); `render_interpreter` validation context = `Unfiltered`.
Red-first (all in `backend_engine.rs` tests, green after):
`root_context_binds_unfiltered_path_prefix_not_sentinel` (red
`Some("__NO_PATH__/")` vs `""`),
`descendants_under_root_binds_no_sentinel_predicate` (red on the sentinel
LIKE), `lookup_block_path_errs_on_missing_block_not_fabricated` (red
`Ok("/block:does-not-exist-9f3a")`); logs `lane-logs/red-41-45.log`,
`lane-logs/red-descendants.log`. GAP NOT FULLY CLOSED: the live descendants
row set is not asserted headlessly (`block_with_path` is unpopulated in
`create_test_engine`); the compile-seam test asserts the bound query text
instead, and the windowed `nested_page_real_engine` probe covers the live
matview path.
