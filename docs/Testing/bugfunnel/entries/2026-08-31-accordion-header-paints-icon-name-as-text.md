---
id: 2026-08-31-accordion-header-paints-icon-name-as-text
date: 2026-08-31
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  The accordion header renders its `icon` prop as the literal string, so the
  linked-references section reads "link Linked references" instead of showing a
  link glyph.
---

## Bug

Found by the `dogfood-explorer` gate looking at a real GPUI window (lane
`dogfood-mobile`, port 8720). Visible in every screenshot of the session at
both widths — `shots/00-baseline.png` (560x850) and `shots/08-wide.png`
(1200x850):

```
▾ link Linked references
```

The word `link` is the icon NAME leaking into the UI as body text, set in the
same face and weight as ordinary content, immediately left of the bold title.
The seed authors it as an icon:

```
accordion(#{title: "Linked references", icon: "link", max_height_fraction: 0.33}, …)
```

## Root cause

`frontends/gpui/src/render/builders/accordion.rs:35-37`:

```rust
if !icon.is_empty() {
    header = header.child(div().child(icon));
}
```

The name is put straight into a `div` as text. Every other header in the tree
routes the name through the `icon()` builder
(`frontends/gpui/src/render/builders/icon.rs`), which maps a name to its glyph
and carries the Android substitution table
(`ICON_SUBSTITUTES`, `frontends/gpui/src/lib.rs:88`). The sidebar's own rows do
this correctly — the seed uses `icon("notebook")` / `icon("sync")` /
`icon("orgmode")` there.

Because the string never reaches `icon()`, it also escapes the icon-font
coverage sweeps that the comment at lib.rs:79-86 describes as the invariant
protecting Android from tofu.

## Missing piece

No assertion on what the accordion HEADER paints.
`frontends/gpui/tests/accordion_sizes_to_content_windowed.rs` measures the
region's height and visible row count; nothing reads the header's text. And the
structured channel cannot see it either — `describe_ui` reports the whole
accordion as `{"widget":"empty"}`
(`2026-08-31-describe-ui-erases-accordion-subtree`), so the `icon` prop is not
observable there.

## Remedy

Open. Route the prop through the shared `icon()` builder so the glyph, the
theme colour and the Android substitution table all apply, and add a header
assertion — text content, not only geometry — to the windowed accordion test.
