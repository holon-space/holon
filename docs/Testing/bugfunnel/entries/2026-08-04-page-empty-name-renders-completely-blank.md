---
id: 2026-08-04-page-empty-name-renders-completely-blank
date: 2026-08-04
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A page with an empty name renders as a completely BLANK sidebar row; the
  `(untitled)` placeholder that exists precisely to prevent this can never
  fire.
source_line: 778
---

## Bug

(dogfood, left sidebar, provoked deliberately after the passive vault showed
no instance) **A page with an empty name renders as a completely BLANK
sidebar row; the `(untitled)` placeholder that exists precisely to prevent
this can never fire.** Repro: tag any empty-content block as a Page
(`execute_operation block.add_tag {tag: "Page"}`) — it immediately appears
in the left sidebar as a row with a bullet and NO text at all (screenshot
`08-empty-page.png`). The shipped sidebar item template already asks for the
placeholder: `text(col("content"), #{empty: "(untitled)"})`
(`block:left_sidebar::render::0`), and the GPUI renderer already implements
it — `frontends/gpui/src/render/builders/text.rs:32` reads
`node.prop_str("empty")` and feeds `holon_api::render_eval::text_display`,
whose unit tests pass. ROOT CAUSE is the shadow builder in between:
`crates/holon-frontend/src/shadow_builders/text.rs:7` declares `fn
text(content, bold, size, color, style)` — **`empty` is not a declared
param**, so the kwarg is dropped at the build boundary and never reaches
`__props`; `prop_str("empty")` is therefore ALWAYS `None` and the disclosed
placeholder is dead code in production. This is a second instance of a class
the file itself documents at lines 17–33: "Before `style` was a declared
param the kwarg was silently dropped and the title rendered at body size."
The same silent-drop happened again with `empty`, which means the class was
closed by adding one param rather than by making an undeclared kwarg an
error. Companion evidence that empty-named pages are a half-broken state
overall: the log shows `page 'block:931f4b12-…' has an EMPTY title and so
contributes no path segment … REFUSING write-back for THIS document` —
correct fail-loud, but it means such a page exists in SQL and the sidebar
while being unrepresentable on disk.

## Missing piece

Nothing composes a Page-tagged block with empty content, so the `empty:`
branch is never rendered by any automated layer. Missing piece = two things:
(i) the generic fix — an undeclared kwarg on a `widget_builder!` widget must
be a LOUD build-time error (or at minimum a warning like the one `style`
already emits for an unknown keyword), so this class cannot recur a third
time silently; (ii) a render case that binds `text(col(...), #{empty: …})`
to an empty value and asserts the placeholder string is displayed. Secondary
ORACLE because even if generated, no invariant relates a rendered row's
displayed text to "non-empty or the disclosed placeholder".

## Remedy

OPEN 2026-08-04 — diagnosis only. FIX DIRECTION: add `empty: Option<String>`
to the `text` shadow builder's params and insert it into `__props`; then
close the class by making unknown kwargs fail loud in `widget_builder!`
rather than fixing widgets one at a time.
