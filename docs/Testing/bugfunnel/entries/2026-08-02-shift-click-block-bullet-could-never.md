---
id: 2026-08-02-shift-click-block-bullet-could-never
date: 2026-08-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Shift+click on a block bullet could never pin the block:
  `assets/default/types/block_profile.yaml` (3 render variants) declared
  `shift_action: focus_pin(#{region: "right", ...})`, but the only accepted
  region literals are `main` / `left_sidebar` / `right_sidebar`
  (`Region::from_str`, `crates/holon-api/src/types.rs:768`). Once the
  `shift_action` template arg resolves at all, the gesture dispatches
  `navigation.focus_pin(region="right")` and the provider rejects it with
  `Invalid region: "right"` before any row is written. The literal was inert
  while `is_template_arg` (`crates/holon-api/src/render_eval.rs`) still
  omitted `shift_action` — the arg resolved to `None`, so nothing was
  dispatched and nothing failed; enabling the template arg turns a silent
  no-op into a loud provider error. Fixed in the same lane (all three variants
  now say `right_sidebar`).
source_line: 1149
---

## Bug

(agent exploration — modifier-click generation lane) Shift+click on a block
bullet could never pin the block: `assets/default/types/block_profile.yaml`
(3 render variants) declared `shift_action: focus_pin(#{region: "right",
...})`, but the only accepted region literals are `main` / `left_sidebar` /
`right_sidebar` (`Region::from_str`, `crates/holon-api/src/types.rs:768`).
Once the `shift_action` template arg resolves at all, the gesture dispatches
`navigation.focus_pin(region="right")` and the provider rejects it with
`Invalid region: "right"` before any row is written. The literal was inert
while `is_template_arg` (`crates/holon-api/src/render_eval.rs`) still
omitted `shift_action` — the arg resolved to `None`, so nothing was
dispatched and nothing failed; enabling the template arg turns a silent
no-op into a loud provider error. Fixed in the same lane (all three variants
now say `right_sidebar`).

## Root cause

agent exploration, modifier-click generation lane — `block_profile.yaml`
passed the invalid region literal `"right"` to `focus_pin` in all three
render variants (`Region::from_str` accepts only
main/left_sidebar/right_sidebar), so shift-click-to-pin dispatches a
provider error once the template arg resolves; inert while the arg was
unlisted, which is why neither bug ever surfaced — no driver click verb
carried modifiers, so the whole YAML→template→dispatch chain had no
automated reader)

## Missing piece

The keystone never generated a click carrying modifiers. Every driver click
verb (`click_entity`, `click_block`, `click_at_element`) was modifier-less,
and the `PinBlock` transition dispatched `navigation.focus_pin` DIRECTLY (a
shortcut its own module doc flagged as `UNFAITHFUL SHORTCUT (audit
TR-NAV)`), so the YAML→`is_template_arg`→`selectable`→dispatch chain had no
automated reader at all. Closed by making `PinBlock`'s SUT body a real
shift+click through the driver plus a modifier-keyed intent lookup
(`focus_path::find_click_intent_in_*` now take `ClickModifiers`), and by the
deterministic case `shift-click-bullet-pins-block-to-right-sidebar` in
`hand-authored-regressions/keystone.jsonl`.

## Remedy

FIXED 2026-08-02 (same lane). Teeth verified by inversion: restoring
`region: "right"` fails with `[PinBlock] shift+click … Invalid region:
"right"`; removing `"shift_action"` from `is_template_arg` fails with
`[inv-focus-roots] expected {"block:c1"} / matview {}`. Residual:
cmd/ctrl-click open-in-tab (`assets/default/index.org`) is still ungenerated
— needs a reference tab model; plan at
`~/.claude/plans/modifier-click-generation-plan.md`.
