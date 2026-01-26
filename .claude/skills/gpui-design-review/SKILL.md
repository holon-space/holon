---
name: gpui-design-review
description: Design-quality checklist for holon-gpui native UI (color, typography, layout, motion, interaction, and an "AI slop" aesthetic-tell list). Use when reviewing, critiquing, or polishing any GPUI screen or component — screenshot the window (see ui-inspection skill), then walk it against this checklist. Triggers on "review the UI", "polish this screen", "does this look AI-generated", "critique the design".
---

# GPUI Design Review

A design-taste checklist distilled from the `impeccable` plugin (web/HTML-oriented) and
re-grounded in what actually exists in `frontends/gpui`: HSLA theme colors
(`theme.colors.*`, see `frontends/gpui/src/lib.rs`), flex-based layout
(`.flex_col()`/`.flex_row()`/`.gap_*()`, no DOM/CSS/grid), and no browser to inspect.
There is no dev server, no HTML, no CSS cascade, and no automated contrast/lint tool for
this stack — this checklist is applied by eye against a screenshot, not by a script.

## Workflow

1. Capture the window: use the **ui-inspection** skill (`CoreGraphics` + `screencapture -l
   <WID>`) since GPUI windows don't register with macOS accessibility APIs or standard
   screenshot tools.
2. Read the PNG with the `Read` tool.
3. Walk the checklist below; call out concrete violations with file:line where the
   offending style call lives (usually in `frontends/gpui/src/**`).
4. Fix by editing the Rust builder chain directly — there is no separate stylesheet.

## Color

- **Contrast**: body text ≥ 4.5:1 against its background; large text (bold, or clearly
  larger than body) ≥ 3:1. The most common failure is muted-gray-on-near-white — if
  contrast is even close, push `text_color` toward the ink end of the ramp rather than
  trusting a "looks elegant" light gray.
- Gray text on a colored/tinted background reads washed out — darken toward the
  background's own hue, or use an alpha-blended text color, not a flat neutral gray.
- Prefer HSLA for any new palette work in this codebase (matches `rgba8_to_hsla` already
  used in `lib.rs`) so hue/lightness/chroma stay independently tunable, same benefit
  OKLCH gives on the web.
- Tinted neutrals: nudge grays slightly toward the brand's own hue rather than a generic
  cool/warm default "because it feels that way" — that default is exactly what makes AI
  output converge on the same palette.
- **Avoid the cream/sand/beige default** for large background surfaces unless it's a
  deliberate, named brand color in this app's theme — that warm-neutral band is the most
  common generic-AI tell, native apps included.
- Pick a **color strategy** before picking colors, and be able to name which one a given
  screen uses: restrained (neutrals + one accent), committed (one saturated color owns
  most of the surface), full palette (3-4 deliberate named roles), or drenched (surface
  IS the color). A screen with no legible strategy usually reads as flat/generic.

## Typography

- Cap body text line length at roughly 65-75 characters; in a flex layout this usually
  means giving text containers a `max_width` rather than letting them fill the window.
- Don't pair two similar-but-not-identical faces (two geometric sans, two humanist sans).
  Contrast on a real axis (serif+sans, geometric+humanist) or vary weight within one
  family.
- Heading scale ceiling: don't push a display heading far past what reads as a heading —
  oversized text shouts rather than establishes hierarchy.
- Vary weight/size deliberately for hierarchy; don't rely on color alone to distinguish
  heading levels.

## Layout

- Flex for 1D arrangement (`flex_row`/`flex_col`), only reach for anything grid-like when
  the layout is genuinely 2D — most GPUI panels are 1D and over-nesting flex containers to
  fake a grid is a sign the layout model is fighting the content.
- Cards are the lazy affordance — reach for a bordered/background container only when
  it's genuinely the best way to group content, never nest a card inside a card.
- Vary spacing (`gap_*`, padding) deliberately for rhythm; uniform spacing everywhere
  reads flat.
- Build a deliberate z-order/layering convention (e.g. base → panel → popover/menu →
  modal → toast/tooltip) instead of ad hoc layering — check how `overlay`/popover-style
  elements in this codebase already establish stacking before adding a new one.

## Motion

- Motion should be planned as part of the change, not bolted on. (Note: `frontends/gpui`
  currently has essentially no animation usage — this is unexplored territory here, not
  an existing convention to match.)
- Ease out with decelerating curves for entrances; avoid bounce/elastic easing.
- Any animation needs a reduced-motion fallback (instant/crossfade) — check whether
  holon's settings expose a reduced-motion or "prefers less motion" flag before adding
  motion that can't be disabled.
- Don't gate content's *initial visibility* on an animation completing — if a reveal
  animation fails to fire (window not focused, dropped frame), the content must still be
  reachable.

## Interaction

- Popover/dropdown-style elements must escape their parent's clipping — if a menu is
  rendered inside a container that clips overflow, it will be cut off; use whatever
  overlay/absolute-positioning primitive GPUI's layout gives for floating elements rather
  than nesting inside a scrollable/clipped parent.
- Every interactive element needs a visible focus/hover state distinguishable at the
  contrast levels above, not just a cursor change.

## Absolute bans — the "AI slop" tells

Match-and-refuse; if a change is about to introduce one of these, restructure it instead.
These are tech-agnostic — they're about visual composition, not CSS mechanics — so they
apply to a native GPUI screen exactly as much as a web page:

- **Side-stripe accents**: a colored bar down one edge of a card/row/callout as the
  "accent." Rewrite with a full border, a background tint, a leading icon, or nothing.
- **Gradient text**: multi-color gradient fills on text for emphasis. Use one solid color;
  carry emphasis with weight/size instead.
- **Glassmorphism as a default**: blur/translucency used decoratively everywhere. Reserve
  for a specific, purposeful moment (e.g. an actual overlay above content), not a default
  card treatment.
- **The hero-metric template**: big number + small label + supporting stats + gradient
  accent, reflexively applied to any "show a number" moment.
- **Identical repeated cards**: same-shaped icon+heading+text card, repeated for every
  item regardless of whether the content actually wants that shape.
- **Tiny uppercase tracked "eyebrow" labels above every section** — a kicker label on
  literally every section is scaffolding-by-reflex, not a chosen brand device.
- **Numbered markers (01/02/03) as default scaffolding** — only legitimate when the
  content is a real, ordered sequence the reader needs to follow in order.
- **Text that overflows its container** — check headings/labels at the window's smallest
  supported size, not just a comfortable default window size.

## The slop test

If someone could look at the screen and say "an AI made that" without hesitation, it's
failed. Two altitudes to check:

- **First-order**: could someone guess the palette/layout from the feature category alone
  (e.g. "it's a task list, so of course it's blue-and-white cards")? If so, the design
  didn't make a real choice.
- **Second-order**: even after avoiding the obvious reflex, is the fallback itself now a
  cliché (e.g. avoided "generic SaaS blue" by defaulting to "generic dark terminal
  aesthetic")? Push one layer past the first dodge.

## What this skill deliberately does NOT cover

No DOM/CSS linting, no automated contrast checker, no in-app "live variant picking," and
no mobile HIG/Material guidance — none of that machinery applies to a native
GPUI/Metal-rendered desktop window. This is a manual checklist applied to a screenshot,
not an automated pipeline.
