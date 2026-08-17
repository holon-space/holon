---
id: 2026-07-20-gpui-turn-into-page-slash-menu
date: 2026-07-20
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  GPUI "Turn into page" (slash menu) silently does nothing: the menu-trigger
  `/` is still in the origin block's content when `convert_block_to_page`
  dispatches, because `editor_view.rs` `ExecuteAndStripCommand` stripped the
  command on a DETACHED async spawn AFTER the synchronous `dispatch_intent`;
  the `block_to_page_plan` planner treated origin content as a `/`-split page
  path, so the trailing `/` → "empty segment" fail-loud; and unlike
  `instantiate_template`, convert took the fire-and-forget dispatch branch (no
  awaitable/toast), so the ERROR was only logged (`holon_frontend::reactive`),
  never shown. Net: converting a block via the slash menu ALWAYS failed and
  looked like a no-op. Headlessly confirmed: convert on content
  ending/containing `/` failed, clean content succeeded.
source_line: 1024
---

## Bug

GPUI "Turn into page" (slash menu) silently does nothing: the menu-trigger
`/` is still in the origin block's content when `convert_block_to_page`
dispatches, because `editor_view.rs` `ExecuteAndStripCommand` stripped the
command on a DETACHED async spawn AFTER the synchronous `dispatch_intent`;
the `block_to_page_plan` planner treated origin content as a `/`-split page
path, so the trailing `/` → "empty segment" fail-loud; and unlike
`instantiate_template`, convert took the fire-and-forget dispatch branch (no
awaitable/toast), so the ERROR was only logged (`holon_frontend::reactive`),
never shown. Net: converting a block via the slash menu ALWAYS failed and
looked like a no-op. Headlessly confirmed: convert on content
ending/containing `/` failed, clean content succeeded.

## Missing piece

keystone `block_to_page` transition dispatches at the op-floor with CLEAN
content, never through CommandProvider→editor_view
async-strip→fire-and-forget; needs a McpUserDriver rung driving convert
through the real slash-menu path (trigger `/` left in), a
content-containing-`/` case, and an inv-no-silent-write-failure oracle

## Remedy

**FIXED THIS LANE (2026-07-20)**: (C) planner + ref-model now mint the page
id via `PageId::for_page_under(destination, leaf)` — content is a
single-segment TITLE, never `/`-split (deterministic red→green test
`convert_treats_slash_in_content_as_title_not_path`); (B)
`ExecuteAndStripCommand` now strips THEN dispatches in ONE ordered spawn;
(D) every menu-dispatched op now takes the `dispatch_intent_awaitable`+toast
path (fail-loud). Backend compound itself was healthy. Coverage-gap
(menu-path McpUserDriver rung + inv-no-silent-write-failure) remains OPEN —
deferred, out of tonight's scope
