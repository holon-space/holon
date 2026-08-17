---
id: 2026-07-16-move-block-happily-reparents-page-under
date: 2026-07-16
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  move_block happily reparents a PAGE under a non-page text block (Testing
  page → plain block; parent_id lands, no error) — violates the just-landed
  "no pages under non-page parents" rule (079014ef) live
source_line: 825
---

## Bug

move_block happily reparents a PAGE under a non-page text block (Testing
page → plain block; parent_id lands, no error) — violates the just-landed
"no pages under non-page parents" rule (079014ef) live

## Missing piece

the PBT invariant (no_page_under_non_page.rs) exists but the
generator/transition set never moves a Page under a non-page; op-level guard
missing entirely

## Remedy

FIXED (2026-07-17) — WRITE-SIDE OP GUARD added at the single shared reparent
chokepoint `BlockOperations::move_block`
(`crates/holon-core/src/traits.rs`): a `Page`-tagged block may only be
reparented under another page; a non-page parent now fails loud (`return
Err(...pages under non-pages are prohibited...)`). Both providers inherit it
(SQL `SqlBlockOperations` + Loro `LoroBlockOperations` use this default
impl; `indent`/`outdent`/`move_up`/`move_down` all route through
`move_block`), so no SqlOnly/Loro asymmetry is possible by construction. The
prod drag-drop path dispatches op `move_block` → this guard
(`user_driver.rs` `DEFAULT_DROP_OP_NAME`). RED-first proven (A/B) by
`move_block_rejects_page_under_non_page` +
`move_block_allows_page_under_page_and_non_page_anywhere`
(`block_operations_tests.rs`): guard-disabled → the move lands and
`expect_err` panics; guard-enabled → op errs, tree untouched. `name_chain`
(`sync_ports.rs:206`) remains the downstream READ-side tripwire.
COMPOSED-PBT gap NOT closed and is an ARCHITECTURE FORK: the guard is only
reachable for a NON-ROOT page (a root page's `parent_id()` is `None` →
`move_block` bails "Cannot move root block" before the guard), but the
composed keystone generator provably only ever creates pages seed-only at
`EntityUri::no_parent()` (see row 156; `ref_caps/docs.rs`), so it
structurally cannot produce a movable non-root page to drive the prohibited
move. Exercising the move-guard in-composition would require seeding a
nested page into the keystone (baseline regression risk) or activating
journals machinery (echo-loop-blocked) — deferred to Martin's ruling; the
`inv-no-page-under-non-page` oracle stays the generator-guarantee regression
guard.
