---
id: 2026-10-09-typed-widget-param-wrong-type-silently-defaulted
date: 2026-10-09
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A `widget_builder!` param given a value of the wrong type was read as absent and drawn at its
  default: `row(#{gap: "zz"})` drew gap 8, `text(#{size: "zz"})` 14, `section(#{title: 5})`
  "Section", `badge(#{label: 0})` an empty label; `row(#{align: "middle"})` drew centred.
---

## Bug
Found by the adversarial verifier of the C1 lane (`c1r4-verify.md`, D-V1, probe2.log) on the
render DSL: a typed param's wrong-typed value built a non-error node whose prop equals the
default.

## Root cause
The macro's extraction (`crates/holon-macros/src/widget_builder.rs`, `ParamType::F32` and
siblings) read params through `get_f64` / `get_string` / `get_bool`, which answer `None` for a
value of another type; the `unwrap_or(default)` that follows cannot tell that from "absent".
`row` copied `align` through and GPUI's `row.rs` matched `_ => items_center()`.

## Missing piece
The authored-args property judged only "no panic"; no property compared a typed param's drawn
prop with the authored value.

## Remedy
`ResolvedArgs::param_string` / `param_f64` / `param_bool` (`crates/holon-api/src/render_eval.rs`)
answer `Err` for a present value of the wrong type (`Null` stays absent); the macro's
`scalar_extraction` draws an error node naming the builder and the key, and
`resolve_props_from_args` returns that `Err` so the props fast path agrees. `row` refuses an
`align` other than start/center/end. Property
`every_typed_param_is_honoured_or_refused` in
`crates/holon-frontend/tests/every_builder_survives_authored_args.rs`; red log
`lane-logs/c1h/dv1-red.log`.
