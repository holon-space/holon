---
id: 2026-07-10-editor-buffer-goes-stale-after-join
date: 2026-07-10
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Editor buffer goes stale after join_block: next click loads pre-join text,
  split fails on length ("Split position 18 exceeds content length 17"),
  set_field commits the resurrected pre-join content — silent data corruption,
  no banner (dogfood #2, pass 1: split → Backspace-join → click merged block →
  Enter at end → type)
source_line: 879
---

## Bug

Editor buffer goes stale after join_block: next click loads pre-join text,
split fails on length ("Split position 18 exceeds content length 17"),
set_field commits the resurrected pre-join content — silent data corruption,
no banner (dogfood #2, pass 1: split → Backspace-join → click merged block →
Enter at end → type)

## Missing piece

CORRECTED after fix: the invariant EXISTS (`inv-displayed-text/widget`
compares live InputState vs ref, its failure message even names this class)
— pure ENVIRONMENT gap: headless keystone has no GPUI window so the /widget
arm self-reports Skipped and /viewmodel can't see InputState; catching rung
= live-MCP twin (`general_e2e_composed_pbt_live_mcp`) with a
join→refocus→type mix + a delta-drop/starvation fault-injection knob (repro
is timing-dependent: cached EditorView must miss the join's broadcast delta)

## Remedy

FIXED (stream 2026-07-10): root cause = Increment G gated the render-path
convergence backstop to no-cell editors only; cached EditorView reused
across the join's rowset rebuild held an InputState that never got the
structural delta and nothing re-read the cell authority on refocus. Fix =
`converge_on_render` gate: cell-attached editors converge from cell
authority (`current_text()`, never SQL content) on the focus-gain edge;
steady state stays cell-only (Increment G `inv-displayed-text` teeth
preserved). Enter-at-end had NO second cause (`split_block` already allows
position==len → empty sibling). Tests:
`cell_editor_converges_only_on_focus_gain`,
`no_cell_editor_keeps_full_backstop` (gpui),
`cell_authority_reflects_merged_content_after_external_join` (frontend,
exact 18→17 join scenario). Live-MCP rung run still open
