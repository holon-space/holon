---
id: 2026-07-18-dioxus-web-frontend-renders-links-all
date: 2026-07-18
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  dioxus-web frontend renders NO links at all — page links (`[[Name]]` /
  `[[id][label]]`) and every other `InlineMark::Link` show up as plain,
  non-clickable text (found by agent code-inspection, latent sibling of the
  GPUI dangling-link-click work). Divergence point: the shared render model
  carries link `marks` on `ViewModel.entity` (same `blocks.marks` source GPUI
  reads), and GPUI's `frontends/gpui/src/render/builders/rendered_text.rs`
  reads them, splits content into text/link segments and wires clicks — but
  dioxus-web's `frontends/dioxus-web/src/render/builders/rendered_text.rs`
  only ever used `ViewKind::RenderedText.content` (a plain `String`) and never
  looked at `node.entity`'s marks, so links were flattened to text. The marks
  were present and correct all along; only this one frontend's builder dropped
  them.
source_line: 1008
---

## Bug

dioxus-web frontend renders NO links at all — page links (`[[Name]]` /
`[[id][label]]`) and every other `InlineMark::Link` show up as plain,
non-clickable text (found by agent code-inspection, latent sibling of the
GPUI dangling-link-click work). Divergence point: the shared render model
carries link `marks` on `ViewModel.entity` (same `blocks.marks` source GPUI
reads), and GPUI's `frontends/gpui/src/render/builders/rendered_text.rs`
reads them, splits content into text/link segments and wires clicks — but
dioxus-web's `frontends/dioxus-web/src/render/builders/rendered_text.rs`
only ever used `ViewKind::RenderedText.content` (a plain `String`) and never
looked at `node.entity`'s marks, so links were flattened to text. The marks
were present and correct all along; only this one frontend's builder dropped
them.

## Missing piece

the ONE keystone PBT renders through the GPUI builders (and drives live iOS
over MCP) but NEVER instantiates the dioxus-web snapshot builders — the
entire `frontends/dioxus-web` render tree is unobserved by any automated
test, so a builder that silently ignores a field the shared model provides
is structurally invisible. Closing the environment gap needs a dioxus-web
render rung (a snapshot-render assertion or a wasm/browser driver in the
keystone), analogous to the live-iOS MCP gate

## Remedy

FIXED (`frontends/dioxus-web/src/render/builders/rendered_text.rs`): the
builder now reads `marks` off `node.entity` (fail-loud JSON parse, same
contract as GPUI) and, when link marks are present, renders each link run as
a distinct clickable element via a NEW shared, host-tested segment splitter
`holon_frontend::link_segments::link_content_segments` (7 unit tests in
`crates/holon-frontend/src/link_segments.rs`: no-links,
non-link-marks-ignored, single/multi link ordering, link-at-start/end,
multibyte char-boundary slicing, dangling `Name` preserved). `Internal`
links dispatch `navigation.focus{region:main, block_id}` (GPUI parity);
`External` render as `<a target=_blank rel=noopener>`; `Name` (dangling)
dispatch `block.create_page_from_link{target}` to create+heal the page. GAP
(noted honestly): same-gesture navigation for DANGLING links is NOT wired —
GPUI's `follow_dangling_link` threads the create op's response (new page id)
into a `navigation.focus`, but the worker exposes no such response-returning
export and `engineDispatchIntents` is fire-and-forget, so dioxus-web
creates+heals the link on first click and navigates on the second (needs an
`engine_follow_dangling_link` worker export to match GPUI's one-gesture
behavior).
