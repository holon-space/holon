---
id: 2026-08-02-arg-never-renders-widget-whole-lazy
date: 2026-08-02
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `expand_toggle`'s `content:` arg NEVER renders — the widget's whole lazy
  body is silently dropped. `is_template_arg`
  (`crates/holon-api/src/render_eval.rs`) decides template-vs-scalar for a
  named arg by a GLOBAL name allowlist, but templateness is a PER-WIDGET
  property: `content` is not on the list, so `resolve_args_with` evaluated it
  against the row into `named`, `ba.args.get_template("content")` answered
  `None`, and `expand_toggle` built a `lazy_slot: None` — no error, no
  warning, an expanded toggle just shows its header. Same root cause and same
  silence as the four modifier-click `*_action` args of `selectable` (fixed
  2026-08-02 by allowlisting the names), which makes this the second sighting:
  any widget arg the DSL author declares but the global list has never heard
  of is dead on arrival, and the failure mode is an empty region rather than a
  diagnostic. Structural, not incidental: 31 of 49 shadow builders are `raw
  fn` over an args bag, contribute NO `WIDGET_META.params`, and therefore have
  no way to state their own arg types.
source_line: 786
---

## Bug

(task #18) `expand_toggle`'s `content:` arg NEVER renders — the widget's
whole lazy body is silently dropped. `is_template_arg`
(`crates/holon-api/src/render_eval.rs`) decides template-vs-scalar for a
named arg by a GLOBAL name allowlist, but templateness is a PER-WIDGET
property: `content` is not on the list, so `resolve_args_with` evaluated it
against the row into `named`, `ba.args.get_template("content")` answered
`None`, and `expand_toggle` built a `lazy_slot: None` — no error, no
warning, an expanded toggle just shows its header. Same root cause and same
silence as the four modifier-click `*_action` args of `selectable` (fixed
2026-08-02 by allowlisting the names), which makes this the second sighting:
any widget arg the DSL author declares but the global list has never heard
of is dead on arrival, and the failure mode is an empty region rather than a
diagnostic. Structural, not incidental: 31 of 49 shadow builders are `raw
fn` over an args bag, contribute NO `WIDGET_META.params`, and therefore have
no way to state their own arg types.

## Missing piece

No rung draws a render-DSL widget call carrying a template arg the allowlist
omits — the generator's DSL alphabet is built from expressions the allowlist
already covers, so an undeclared template arg is unreachable by
construction. ORACLE secondary and independent: nothing asserts that an arg
a widget READS is an arg the resolver KEPT, so even had the shape been
generated, a silently-empty lazy slot would have judged green.

## Remedy

FIXED 2026-08-02 — arg classification is now per-widget.
`WidgetMeta::classifies_as_template` answers from the widget's own declared
params; `is_template_arg_for(widget, name)` consults it first and only then
the global allowlist; `resolve_args_for_widget` threads the widget's meta
from `RenderInterpreter::interpret` (which already had the callee name in
hand) through `RenderInterpreter::set_widget_metas`, seeded from the
macro-generated `all_widget_metas()`. `expand_toggle` and `selectable`
migrated from `raw fn` to the macro's typed-params-with-body form (`fn
expand_toggle(header: Expr, content: Expr)`, `fn selectable(action: Expr,
shift_action: Expr, cmd_action: Expr, ctrl_action: Expr, alt_action:
Expr)`), so both now declare their template args and need NO allowlist entry
— an arg used but not declared is a compile error (`E0425: cannot find
value` on the missing binding), not a silent `None`. Backstop against the
whole class: `ResolvedArgs::get_template` now PANICS naming widget and arg
when asked for a name that is neither declared nor allowlisted, so a lookup
that could only ever answer `None` fails loud instead of rendering nothing.
Red-first proof:
`shadow_builders::expand_toggle::tests::content_template_materialises_when_expanded`
red on the unmodified tree at `content: must build a lazy slot`, green
after;
`render_eval::mutation_gap_tests::get_template_for_an_unclassified_name_panics`
covers the backstop and `declared_params_override_the_global_allowlist` pins
both precedence directions. The fix UNMASKED a dormant hole it had been
hiding: with `content` dead, no test had ever driven a lazy slot through
`StubBuilderServices`, whose `clone_arc` was still the trait's
`unimplemented!()` — 4 frontend tests panicked the moment content started
materialising; `StubBuilderServices` now returns a real handle. Residual:
the remaining template-reading raw widgets (`board`, `columns`, `outline`,
`tree`) still ride the allowlist for `item_template`/`item`, because those
args are read by shared helpers in `render_interpreter.rs` off `ba.args`
rather than by the widget bodies — retiring those two names needs the
helpers to take the templates as parameters first.
