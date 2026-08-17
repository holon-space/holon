---
id: 2026-07-30-org-headline-owns-query-source-child
date: 2026-07-30
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  An org headline that owns a query-source child renders with NO TITLE TEXT:
  on the `ClaudeCode` page the root `Claude Code History` renders normally but
  its five sections — `Conversation`, `Projects`, `Recent Sessions`, `Sessions
  (Chat View)`, `Tasks` — render as five title-less rows. Not an ingest
  defect: a headless boot of `Projects/Holon/ClaudeCode.org` produces all five
  section blocks with correct `content` and `parent_id` and `source_language
  IS NULL`, and the live DB agrees. It is the render profile.
  `assets/default/types/block_profile.yaml:135-137` defines variant
  `query_block` with condition `has_query_source` and render `live_block()` —
  a bare widget, with no `rendered_text(col("content"))` and no bullet, unlike
  the `default` variant at line 149. `has_query_source` (line 34) is true for
  any block owning a `holon_prql`/`holon_sql`/`holon_gql`/`render` child,
  which every one of the five sections does (`cc-projects::src::0` etc.).
  Confirmed live over the `holon` MCP: `describe_ui block:cc-history-root` →
  `rendered_text "Claude Code History"`; `describe_ui block:cc-projects` →
  `view_mode_switcher > list [42 items]`, no title node anywhere. Suppression
  is CORRECT for the seeded layout blocks it was written for
  (`default-main-panel` must not print "Main Panel") but wrong for
  user-authored content headlines, which become unreadable and unfocusable.
  Aggravating co-factor at Martin’\s observation: on a fresh migration the
  `cc_*` tables are not yet populated, so the widgets were empty too and the
  rows were entirely blank.
source_line: 1127
---

## Bug

An org headline that owns a query-source child renders with NO TITLE TEXT:
on the `ClaudeCode` page the root `Claude Code History` renders normally but
its five sections — `Conversation`, `Projects`, `Recent Sessions`, `Sessions
(Chat View)`, `Tasks` — render as five title-less rows. Not an ingest
defect: a headless boot of `Projects/Holon/ClaudeCode.org` produces all five
section blocks with correct `content` and `parent_id` and `source_language
IS NULL`, and the live DB agrees. It is the render profile.
`assets/default/types/block_profile.yaml:135-137` defines variant
`query_block` with condition `has_query_source` and render `live_block()` —
a bare widget, with no `rendered_text(col("content"))` and no bullet, unlike
the `default` variant at line 149. `has_query_source` (line 34) is true for
any block owning a `holon_prql`/`holon_sql`/`holon_gql`/`render` child,
which every one of the five sections does (`cc-projects::src::0` etc.).
Confirmed live over the `holon` MCP: `describe_ui block:cc-history-root` →
`rendered_text "Claude Code History"`; `describe_ui block:cc-projects` →
`view_mode_switcher > list [42 items]`, no title node anywhere. Suppression
is CORRECT for the seeded layout blocks it was written for
(`default-main-panel` must not print "Main Panel") but wrong for
user-authored content headlines, which become unreadable and unfocusable.
Aggravating co-factor at Martin’\s observation: on a fresh migration the
`cc_*` tables are not yet populated, so the widgets were empty too and the
rows were entirely blank.

## Missing piece

No composed-keystone transition or generator ever creates a query-source
child block — `holon_prql` appears nowhere under
`crates/holon-integration-tests/src/pbt/composed/`, only in
`test_environment.rs` and the frontend slice — so the only blocks that reach
`query_block` in a keystone run are the seeded layout blocks, for which
suppression is the desired behaviour. No case can distinguish. ORACLE
secondary: even given the state, no invariant asserts that a content
headline’\s own title is present in the rendered row. Missing piece = a
transition that attaches a query source to an ordinary content headline,
plus an invariant over rendered title text.

## Remedy

OPEN 2026-07-30 — diagnosed read-only, NOT fixed. Separate defect from the
quarantine row above; the two symptoms on this page have independent causes.
Needs a ruling from Martin on the intended semantics (title + widget, or
widget only for layout blocks and title + widget for content blocks) before
a fix.
