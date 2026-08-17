---
id: 2026-08-02-modifier-click-action-render-dsl-dead
date: 2026-08-02
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  EVERY modifier-click action in the render DSL is dead, and always has been.
  `is_template_arg` (`crates/holon-api/src/render_eval.rs:648`, allowlist at
  `:681-696`) decides whether a NAMED arg is kept as an unevaluated TEMPLATE
  or evaluated to a scalar, and it is keyed GLOBALLY by arg name against a
  hardcoded list. `action` is on that list; `shift_action`, `cmd_action`,
  `ctrl_action` and `alt_action` are NOT. `selectable`
  (`crates/holon-frontend/src/shadow_builders/selectable.rs:93-106`) looks all
  four up via `ba.args.get_template(..)`, which therefore returns `None` for
  every one of them, so no modifier-triggered `OperationWiring` is ever built
  and the click silently does nothing. VERIFIED FIRSTHAND against the SHIPPED
  expression rather than a synthetic one: `assets/default/index.org:12`
  through `parse_render_dsl` + `resolve_args` yields
  `get_template("action").is_some() == true` while `cmd_action` and
  `ctrl_action` are both `false`. TWO shipped affordances are affected, not
  one: (i) cmd-click / ctrl-click "open page in a new tab" in the DEFAULT left
  sidebar (`assets/default/index.org:12`), and (ii) shift-click "pin block to
  the right region" (`shift_action: focus_pin(..)`), declared in THREE render
  strings in `assets/default/types/block_profile.yaml:148,156,163` — so the
  dead family reaches the block bullet of every default profile view, not just
  the sidebar. Neither has worked in any shipped build. These four are the
  only prod `get_template` lookups outside the allowlist apart from `content`
  (see the `expand_toggle` row), which shares the root cause but is a separate
  architecture fork.
source_line: 1148
---

## Bug

(agent exploration, outside any automated test) EVERY modifier-click action
in the render DSL is dead, and always has been. `is_template_arg`
(`crates/holon-api/src/render_eval.rs:648`, allowlist at `:681-696`) decides
whether a NAMED arg is kept as an unevaluated TEMPLATE or evaluated to a
scalar, and it is keyed GLOBALLY by arg name against a hardcoded list.
`action` is on that list; `shift_action`, `cmd_action`, `ctrl_action` and
`alt_action` are NOT. `selectable`
(`crates/holon-frontend/src/shadow_builders/selectable.rs:93-106`) looks all
four up via `ba.args.get_template(..)`, which therefore returns `None` for
every one of them, so no modifier-triggered `OperationWiring` is ever built
and the click silently does nothing. VERIFIED FIRSTHAND against the SHIPPED
expression rather than a synthetic one: `assets/default/index.org:12`
through `parse_render_dsl` + `resolve_args` yields
`get_template("action").is_some() == true` while `cmd_action` and
`ctrl_action` are both `false`. TWO shipped affordances are affected, not
one: (i) cmd-click / ctrl-click "open page in a new tab" in the DEFAULT left
sidebar (`assets/default/index.org:12`), and (ii) shift-click "pin block to
the right region" (`shift_action: focus_pin(..)`), declared in THREE render
strings in `assets/default/types/block_profile.yaml:148,156,163` — so the
dead family reaches the block bullet of every default profile view, not just
the sidebar. Neither has worked in any shipped build. These four are the
only prod `get_template` lookups outside the allowlist apart from `content`
(see the `expand_toggle` row), which shares the root cause but is a separate
architecture fork.

## Missing piece

The keystone cannot GENERATE a modifier-click. Modifiers exist in the
harness for KEYSTROKES only (`send_raw_keystroke(key, modifiers)`,
`crates/holon-integration-tests/src/pbt/driver_input.rs:358`; `has_modifier`
in `transitions/press_key.rs:172`); no transition or driver anywhere issues
a CLICK carrying a modifier, and `navigation_open_tab` appears in zero test
files, so no generated run can reach the code path at all. Secondary ORACLE,
and the more valuable half: nothing asserts that a template arg a widget
LOOKS UP actually RESOLVED, so an arg that silently degrades to `None` is
invisible to the entire invariant catalog regardless of generation. The
concrete missing oracle is a check of resolved templates against each
widget's DECLARED arg set — exactly what `is_template_arg_from_metas`
(`crates/holon-api/src/widget_meta.rs:58`) enables, since it derives
templateness PER-WIDGET from `WidgetMeta.params` instead of from one global
name list. That global-vs-per-widget keying is the shared root cause of this
class.

## Remedy

FIXED 2026-08-02 — the four names added to the `is_template_arg` allowlist;
`selectable`'s "adding a modifier is one table row" comment now also names
the allowlist, since omitting it is precisely what left the affordance dead.
RED (right reason, unmodified tree): `` `cmd_action` did not resolve as a
template, so the modifier-click action is dead `` from
`render_eval::tests::shipped_left_sidebar_resolves_modifier_click_action_templates`,
which parses `assets/default/index.org` verbatim; the same test asserts
`action` (allowlisted) DOES resolve and that assertion PASSED in the red
run, so the failure is specific to the modifier names and not a malformed
fixture. GREEN after. Deliberately NOT generalized to an
`ends_with("_action")` suffix rule: `max_height_fraction`
(`crates/holon-frontend/src/shadow_builders/accordion.rs:113`) is a scalar
`f64` whose name ends in "action", so a suffix rule would silently convert a
working scalar into a template. RESIDUAL: the GENERATION gap is not closed —
there is still no modifier-click transition, so this is guarded by a
targeted unit test rather than by the keystone.
