---
id: 2026-07-10-design-gallery-hover-never-reveals-example
date: 2026-07-10
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  design_gallery on_hover never reveals: example rebuilds ReactiveViewModel
  per frame, resetting per-node `hovered` Mutable
source_line: 846
---

## Bug

design_gallery on_hover never reveals: example rebuilds ReactiveViewModel
per frame, resetting per-node `hovered` Mutable

## Missing piece

example embedder diverges from prod (per-frame `mode_view_model` vs
persistent `root_vm` + reconcile); no mouse-move/hover primitive in any
driver alphabet

## Remedy

FIXED (per-mode VM cache mirroring root_vm) +
`on_hover_state_survives_only_when_node_is_reused` pins the mechanism; hover
driver rung still open
