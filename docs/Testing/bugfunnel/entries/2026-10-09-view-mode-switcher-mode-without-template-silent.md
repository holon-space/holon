---
id: 2026-10-09-view-mode-switcher-mode-without-template-silent
date: 2026-10-09
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A `view_mode_switcher` whose `default_mode` had no template drew another mode while marking
  `default_mode` active, and a click on a listed mode without a `mode_*` template did nothing.
---

## Bug
Found by the adversarial verifier of the C1 lane (`c1r4-verify.md`, D-V4, D-V5, probe.log P2,
P7). Reachable from hand- or agent-authored render DSL; the backend's
`view_mode_switcher_from_variants` always emits one template per mode.

## Root cause
`crates/holon-frontend/src/shadow_builders/view_mode_switcher.rs` fell back to the first
`mode_*` template while `mark_active_mode` got `default_mode`; `modes:` was parsed leniently
(`unwrap_or("[]")`, `.ok()`), and GPUI's click
(`frontends/gpui/src/render/builders/view_mode_switcher.rs`) had no `else` for a missing or
undecodable `tmpl_mode_*` prop.

## Missing piece
The click leg of the property called `switch` with the raw expression, skipping GPUI's JSON
round trip and missing-template branch; no generator produced a default mode without its
template.

## Remedy
The builder parses `modes:` strictly (`parse_view_modes`) and refuses a listed mode without a
template, a template for an unlisted mode, and a `default_mode` that names no listed mode.
`ViewModeSwitch::switch_mode` decodes and validates the stored template and draws an error node
in the slot when it cannot; GPUI's click calls it. Property
`a_view_mode_switcher_draws_the_mode_it_marks_active`; red logs
`lane-logs/c1h/dv2-dv4-red.log`, `lane-logs/c1h/dv4-dv5-red.log`.
