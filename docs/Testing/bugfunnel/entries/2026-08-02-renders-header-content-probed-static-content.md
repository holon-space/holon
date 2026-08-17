---
id: 2026-08-02-renders-header-content-probed-static-content
date: 2026-08-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `expand_toggle(#{default_expanded: true, ..})` renders its header but NOT
  its content. Probed with STATIC content to rule the query layer out:
  `expand_toggle(#{default_expanded: true, header: text(col("id")), content:
  text("HELLO-STATIC")})` shows the header alone, in `describe_ui` and on
  screen. That probe was sound and is what makes the real cause findable.
  ORIGINAL HYPOTHESIS, WRONG: that a gate seeded open never transitions, i.e.
  a `default_expanded` / `materialize_if_gated` sequencing defect. REFUTED by
  the snapshot itself — it prints `expand_toggle(▼ block:page-a)`, and the ▼
  means the gate IS open, so `materialize_if_gated` did fire. The content is
  missing because THE SLOT WAS NEVER BUILT. ACTUAL CAUSE:
  `crates/holon-frontend/src/shadow_builders/expand_toggle.rs:81` calls
  `get_template("content")`, but `"content"` is absent from the template-arg
  allowlist in `crates/holon-api/src/render_eval.rs:681-696` (`item_template |
  item | header | header_template | child_template | action | parent_id |
  sortkey | sort_key | context | states`, plus `mode_*`).
  `is_template_arg("content")` returns false, so the arg is parsed as a
  SCALAR, `get_template` returns `None`, `lazy_slot` is ALWAYS `None`, and
  content never renders in ANY state — collapsed or expanded, clicked or not,
  static or query-backed. `"header"` IS on that list, which is precisely why
  the header renders and the content does not. So this is not a
  `default_expanded` bug at all and the builder's doc comment at `:31-36`
  describes a code path that cannot execute. THE CLASS, not the instance:
  `is_template_arg` (`render_eval.rs:648`) is keyed GLOBALLY BY ARG NAME while
  templateness is inherently PER-WIDGET, so one name cannot be both a template
  and a scalar. `"content"` is declared a scalar `String` by four widgets —
  `text` (`shadow_builders/text.rs:7`), `source_editor` (`:4`),
  `editable_text` (`:7`), `rendered_text` (`:7`) — with a shipped usage at
  `crates/holon-profiles/src/lib.rs:1339` (`row(text(#{content:
  col("content")}))`), so simply allowlisting `"content"` would divert those
  into `templates` and BLANK them. The same root cause kills a second,
  unrelated affordance: `shadow_builders/selectable.rs:93-96` looks up
  `"shift_action"`, `"cmd_action"`, `"ctrl_action"`, `"alt_action"`, none of
  which is allowlisted, so cmd-click / ctrl-click 'open in tab' is silently
  dead in the SHIPPED left sidebar (proven by parsing
  `assets/default/index.org:12` verbatim: `action` resolves, `cmd_action` and
  `ctrl_action` do not).
source_line: 1147
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710;
ROOT CAUSE CORRECTED after a later lane reproduced it as a unit test — the
observation below was right, the diagnosis was not.)
`expand_toggle(#{default_expanded: true, ..})` renders its header but NOT
its content. Probed with STATIC content to rule the query layer out:
`expand_toggle(#{default_expanded: true, header: text(col("id")), content:
text("HELLO-STATIC")})` shows the header alone, in `describe_ui` and on
screen. That probe was sound and is what makes the real cause findable.
ORIGINAL HYPOTHESIS, WRONG: that a gate seeded open never transitions, i.e.
a `default_expanded` / `materialize_if_gated` sequencing defect. REFUTED by
the snapshot itself — it prints `expand_toggle(▼ block:page-a)`, and the ▼
means the gate IS open, so `materialize_if_gated` did fire. The content is
missing because THE SLOT WAS NEVER BUILT. ACTUAL CAUSE:
`crates/holon-frontend/src/shadow_builders/expand_toggle.rs:81` calls
`get_template("content")`, but `"content"` is absent from the template-arg
allowlist in `crates/holon-api/src/render_eval.rs:681-696` (`item_template |
item | header | header_template | child_template | action | parent_id |
sortkey | sort_key | context | states`, plus `mode_*`).
`is_template_arg("content")` returns false, so the arg is parsed as a
SCALAR, `get_template` returns `None`, `lazy_slot` is ALWAYS `None`, and
content never renders in ANY state — collapsed or expanded, clicked or not,
static or query-backed. `"header"` IS on that list, which is precisely why
the header renders and the content does not. So this is not a
`default_expanded` bug at all and the builder's doc comment at `:31-36`
describes a code path that cannot execute. THE CLASS, not the instance:
`is_template_arg` (`render_eval.rs:648`) is keyed GLOBALLY BY ARG NAME while
templateness is inherently PER-WIDGET, so one name cannot be both a template
and a scalar. `"content"` is declared a scalar `String` by four widgets —
`text` (`shadow_builders/text.rs:7`), `source_editor` (`:4`),
`editable_text` (`:7`), `rendered_text` (`:7`) — with a shipped usage at
`crates/holon-profiles/src/lib.rs:1339` (`row(text(#{content:
col("content")}))`), so simply allowlisting `"content"` would divert those
into `templates` and BLANK them. The same root cause kills a second,
unrelated affordance: `shadow_builders/selectable.rs:93-96` looks up
`"shift_action"`, `"cmd_action"`, `"ctrl_action"`, `"alt_action"`, none of
which is allowlisted, so cmd-click / ctrl-click 'open in tab' is silently
dead in the SHIPPED left sidebar (proven by parsing
`assets/default/index.org:12` verbatim: `action` resolves, `cmd_action` and
`ctrl_action` do not).

## Missing piece

Nothing asserts that a widget's declared template args actually RESOLVE. A
template arg that silently degrades to `None` produces a structurally valid
render with a missing subtree, which every existing test accepts: the
builder tests exercise `expand_toggle` through paths that supply children by
other means, and no test parses a shipped expression and checks that each
arg the builder will ask for is retrievable. The catching assertion is a
cross-check between each widget's declared arg set and the allowlist —
exactly what `crates/holon-api/src/widget_meta.rs:58
is_template_arg_from_metas` exists to make possible, documented there as
'Replaces the hardcoded `is_template_arg()`'. A cheap interim guard is a
test that walks every literal-name `get_template("…")` call site (73 across
`crates/` + `frontends/`) and fails on any name the allowlist omits.

## Remedy

FIXED 2026-08-02 via Martin's ruling (typed-params-with-body migration, task
#18 — full fix record in the task #18 row): classification is per-widget,
the reproducing test is un-`#[ignore]`d and green, and `get_template` panics
on any undeclared lookup. The fork options as they stood before the ruling,
for the record: (a) migrate to per-widget templateness via
`is_template_arg_from_metas`; or (b) a per-widget exception for `content`
now and migrate later — smaller, but adds a second special case to the very
mechanism whose global keying caused this. IMPORTANT CAVEAT discovered by
the `*_action` lane, which changes the size of (a):
`is_template_arg_from_metas` would NOT have caught EITHER of these bugs as
things stand, because both `selectable` and `expand_toggle` are `raw fn`
widgets that read their args by hand and therefore contribute NO
`WidgetMeta.params` for the metas function to consult. Migrating closes the
class only if the raw widgets FIRST declare their template args as metadata,
so (a) is 'declare metadata for the raw widgets, then migrate', not 'swap
one function for another'. The `*_action` half needs NO ruling (no widget
takes those names as scalars) and is being fixed separately with its own
row. NOTE: once `describe_ui` expansion lands, an unrendered `expand_toggle`
content subtree reports an explicit `content_deferred` marker instead of a
bare header — that makes this bug VISIBLE, it does not fix it.
