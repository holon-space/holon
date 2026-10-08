---
id: 2026-10-08-op-button-panics-on-authored-input
date: 2026-10-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `op_button` panicked when its row had no `name`/`target_id`, so an authored render could abort the
  render pass (and a release build) instead of drawing an error.
---

## Bug
Found by code audit in the boot-always design (recovery-layout note, gap G4) and
confirmed in lane C1: `op_button()` over an empty row panicked at
`crates/holon-frontend/src/shadow_builders/op_button.rs:25`
(`lane-logs/c1/red-g4-r2.log`).

## Root cause
Two `.expect(...)` calls on authored input in the shadow builder.

## Missing piece
The keystone only renders `op_button` from `chain_ops`/`ops_of` rows, which
always carry both columns; no case authors it elsewhere.

## Remedy
The builder returns `ViewModel::error("op_button", …)` naming the missing
column. Pinned by `crates/holon-frontend/tests/default_assets_render_ready.rs`,
which also parses and renders every `assets/default` org file. Other builders
that still panic on authored input (`table`, `columns`, `state_toggle`) remain
open.
