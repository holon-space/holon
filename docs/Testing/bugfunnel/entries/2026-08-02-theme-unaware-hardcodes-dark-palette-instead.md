---
id: 2026-08-02-theme-unaware-hardcodes-dark-palette-instead
date: 2026-08-02
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  `chat_bubble` is theme-unaware: it hardcodes a dark palette (`USER_BUBBLE
  0x2A3A3A`, `ASSISTANT_BUBBLE 0x2A2A28`, `TEXT_PRIMARY 0xE8E6E1`,
  `frontends/gpui/src/render/builders/chat_bubble.rs:3-9`) instead of reading
  `ctx.theme().colors` like every other builder (`tc(..)`,
  `builders/prelude.rs:13`). On the default LIGHT theme a conversation renders
  as a column of near-black slabs. The assistant avatar is also a hardcoded
  literal `"H"` regardless of sender or model.
source_line: 1143
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710)
`chat_bubble` is theme-unaware: it hardcodes a dark palette (`USER_BUBBLE
0x2A3A3A`, `ASSISTANT_BUBBLE 0x2A2A28`, `TEXT_PRIMARY 0xE8E6E1`,
`frontends/gpui/src/render/builders/chat_bubble.rs:3-9`) instead of reading
`ctx.theme().colors` like every other builder (`tc(..)`,
`builders/prelude.rs:13`). On the default LIGHT theme a conversation renders
as a column of near-black slabs. The assistant avatar is also a hardcoded
literal `"H"` regardless of sender or model.

## Missing piece

Visual/UX; no formal invariant. A cheap partial oracle would be a lint/test
that no GPUI builder constructs a color literal outside `style.rs` —
chat_bubble is currently the only builder that does.

## Remedy

OPEN — diagnosis only. Fix is mechanical: route the five constants through
`tc(ctx, ..)`.
