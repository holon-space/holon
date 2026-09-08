---
id: 2026-09-08-toast-lines-run-off-the-right-window-edge
date: 2026-09-08
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Toast lines do not wrap and are cut off flush at the right window edge, so the
  pairing disclosure's conflict-copies query and the retry refusal's block list
  are both unreadable — the two strings D93.a and D94.a added exist only in the
  parts of the toast that are off screen.
---

## Bug

Found by the `dogfood-explorer` gate for D93.a / D94.a, port 8720, fixture
`/tmp/dogfood-w10-degraded`. Two different toasts, one defect.

The pairing disclosure toast reads, on screen
(`lane-logs/dogfood-w10/04c-toast-s.png`):

    ℹ Content kept from this device — 1 block(s) written on this device were ad
    Find the copies with: SELECT id, content FROM block WHERE json_extract

Both lines stop flush at the window's right border. The first loses the archive
path; the second loses the whole predicate of the query — the user is told
where to look and then shown two thirds of the way to look. The query is
137 characters and there is no wrap, no ellipsis, no scroll, and no copy
affordance, so it cannot be recovered from the UI at all.

The retry refusal is the same
(`lane-logs/dogfood-w10/05-retry-s.png`):

    ⛔ Command failed — Operation 'pair_retry_reimport' on entity 'device' fa

D94.a's refusal does name every deferred block and its wanted parent — the MCP
call returns the full text — but the user sees none of it. What reaches the
screen is the word "failed", cut mid-word.

## Root cause

`render_toast_stack` (`frontends/gpui/src/share_ui.rs:2086`) positions the
stack `.absolute().bottom(16).right(16)` and gives each toast no width bound
and no wrapping treatment, so a toast is as wide as its longest line and the
overflow leaves the window.

The two strings that overflow were both added deliberately uncapped:
`toast_lines` appends the conflict query as its own line specifically to escape
the `detail` cap (`share_ui.rs:1976`, and the comment at `share_ui.rs:384`),
and the refusal arrives as a `CommandFailed` detail. The 80-byte cap that entry
`2026-08-17-disclosure-tested-before-rendering-truncated-the-remedy` fixed
bounded the string; nothing bounds the box.

## Missing piece

`share_ui.rs`'s own unit tests assert on the STRINGS `toast_lines` returns —
`the toast must paint the whole query` (`share_ui.rs:2644`) is satisfied by a
`Vec<String>`, never by a pixel. No windowed test opens a toast, and no
assertion anywhere relates a toast's rendered width to the window's.

## Remedy

Open. Bound the toast's width against the window and wrap (or ellipsize with a
copy action for the query), then pin it with a windowed assertion that a toast's
painted bounds lie inside the window for a maximal-length line. The conflict
query in particular wants a copy affordance rather than text the user must
retype.
