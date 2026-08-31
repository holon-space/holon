---
id: 2026-08-31-claude-history-chat-views-bind-a-single-message
date: 2026-08-31
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Three `live_query` chat views in the bundled claude-history sidecar pass a
  bare `chat_bubble(...)` as `item_template`, which renders one message
  instead of the conversation — the same scalar-template defect fixed once in
  the integrations section.
---

## Bug

Found by code audit during the 2026-08-31 `ClaudeCode` page investigation, not
by observation. It is **latent**: it is masked today because the
`claude-history` provider never connects
(`2026-08-31-bundled-sidecar-hardcodes-developer-local-binary-path`), so no row
ever reaches these templates. It will surface the moment the integration works.

`assets/integrations/claude-history.yaml` lines 102, 233 and 365 all read:

```yaml
content: live_query(#{
  sql: "SELECT uuid, role, content, timestamp AS ts FROM cc_message ...",
  item_template: chat_bubble(#{sender: col("role"), time: col("ts")}, text(col("content")))
})
```

covering the live-session chat, the session `default` variant and the agent
`default` variant. Expanding any session would show its first message only.

The `ClaudeCode` page routes into this: its `cc-sessions-chat` block is
`list(#{item_template: render_entity()})` over `cc_session`, and each rendered
session entity uses the sidecar's `default` variant — line 233. The page's own
`cc-conversation` block is correct (it wraps the bubble in `list(#{...})`).

## Root cause

Identical mechanism to `2026-08-18-integrations-section-renders-one-of-four-rows`,
recurring at new sites. `live_query`'s `item_template` is not a per-row
template despite its name: `shared_live_query_build`
(`crates/holon-frontend/src/render_interpreter.rs:740-745`) stores it as the
`render_expr` for the WHOLE result, defaulting to `table()` when absent, and
the platform layer interprets it exactly once against all delivered rows
(`frontends/gpui/src/views/reactive_shell.rs:387-388` —
`interpret_pure(&render_expr, &data_rows, services)`).

Only a collection widget (`list`, `table`, `tree`, `outline`, `columns`)
iterates the rows. A scalar widget such as `chat_bubble` renders one instance,
with `col(...)` reading the first row. Production code that gets this right
wraps it — `crates/holon-app/src/integrations_section.rs:43,64` pass
`list(#{item_template: ...})` as the value.

## Reproduced

Red, headless, in 18 s — no running app and no live vault needed. Probe test
appended to `crates/holon-frontend/tests/chat_view_render.rs`
(`probe_two_messages_render_two_bubbles`, left uncommitted in the
investigation lane): it reads the template out of the real sidecar, then does
what the platform layer does — one `interpret_pure(&template, &rows, ...)`
against TWO delivered message rows — and counts bubbles.

```
assertion `left == right` failed: two delivered messages must render two
chat_bubbles; got 1
chat_bubble(user)
  text "FIRST-MESSAGE"
  left: 1
 right: 2
```

The second message is not truncated or errored; it is simply never rendered.
This probe is the red rung the landing fix should turn green.

## Missing piece

The 2026-08-18 fix corrected one call site but added no guard against the
parameter being misused elsewhere. Nothing rejects, warns about, or tests for
a scalar `item_template` on `live_query`, and the sidecar YAML is not checked
against the render DSL's collection/scalar distinction at all.

The sharpest form of the gap is in the neighbouring test. `chat_view_render.rs`
has `message_row_renders_as_a_chat_bubble_with_its_text` (`:315-349`), which
pulls the same template out of the same sidecar — and then interprets it
against exactly **one** fixture row and asserts the bubble carries that row's
text. A one-row oracle cannot distinguish a per-row template from a
whole-result one, so this defect passes the test written to cover it. Every
existing assertion in the file is one-row or presence-only.

Not observable from the log: a scalar template produces a plausible-looking
single bubble, no error and no warning. That is exactly the silent-degradation
mode the repo's error philosophy forbids.

## Remedy

FIXED. Both the instances and the class.

**The class.** `shared_live_query_build`
(`crates/holon-frontend/src/render_interpreter.rs:740-765`) now refuses a
non-collection `item_template` at the DSL boundary, naming the offending widget
and the fix:

> `[live_query item_template must be a collection widget
> (list/tree/table/board/columns/outline); got chat_bubble — a bare per-row
> template binds only the first row. Wrap it: list(#{item_template:
> chat_bubble(…)})]`

The predicate is `collection_layout::is_layout`, the same registry the layout
engine uses, so a newly registered layout is accepted without touching this
site. An absent template still defaults to `table()`, which is a collection.

**The instances.** A census over every `live_query(#{…})` in the repo found
**ten** bare templates, not the three reported:

| site | template | note |
|---|---|---|
| `assets/integrations/claude-history.yaml:102,233,365` | `chat_bubble` | the three |
| `crates/holon-mcp-mock/tests/fixtures/vtable_*.yaml` ×3 | `row` | vtable-contract fixtures |
| `crates/holon-mcp-client/tests/sidecar_drift.rs:47` | `text` | |
| `frontends/gpui/tests/accordion_{hides_when_empty,sizes_to_content}_windowed.rs` | `text` | stale hand-copies of the linkedrefs panel |
| `frontends/gpui/tests/plain_path_scroll.rs:53` | `text` | the LEGACY persisted `__default__.org` shape |

All ten are wrapped in `list(#{item_template: …})`. The three GPUI ones were
hand-written approximations of a production render that already used
`list(...)` (`assets/default/index.org:23`), so wrapping restored parity rather
than changing intent.

**Rungs** (all in `crates/holon-frontend/tests/chat_view_render.rs`):

- `live_query_refuses_a_non_collection_item_template` — the refusal. RED before
  the fix (built a `live_query` wrapping one `chat_bubble` instead of erroring).
- `live_query_accepts_a_collection_item_template` and
  `live_query_without_an_item_template_still_builds` — guard the refusal
  against over-reach; both green before and after.
- `every_chat_variant_renders_one_bubble_per_message` — two rows through
  `interpret_pure`, across all three sidecar variants (`live_session`,
  `session`, `agent`), so pinning one cannot let the others rot.
- `message_row_renders_as_a_chat_bubble_with_its_text` — widened from one row
  to two. This is the oracle repair: the one-row form passed the bug.

The `descendants` walker in that file was also extended to traverse
`collection.children_snapshot()`. It previously walked only `children` and
`slot`, so it could not see per-row widgets at all — a second reason a
collection/scalar distinction was invisible to these tests.

**Still open, deliberately:** a static check over `assets/**` asserting the same
property at authoring time. The runtime refusal covers it for anything that
renders, but a never-rendered asset can still ship bare.
