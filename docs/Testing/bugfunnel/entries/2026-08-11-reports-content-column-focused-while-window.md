---
id: 2026-08-11-reports-content-column-focused-while-window
date: 2026-08-11
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  `describe_ui` reports the CONTENT column for a focused `editable_text` while
  the window shows the source projection.
source_line: 734
---

## Bug

(task #68 dogfood re-entry gate; found by DOGFOODING the live GPUI app; no
automated test produced it) **`describe_ui` reports the CONTENT column for a
focused `editable_text` while the window shows the source projection.** At
one instant the screenshot reads `TODO existing task` and `describe_ui`
reports `editable_text "existing task"`. The rebuilt feature's whole premise
is that the editable surface IS vault syntax, and the MCP surface an agent
drives the app through cannot see that surface — every check of a projection
in this session had to go through a screenshot and pixel arithmetic.

## Root cause

task #68 dogfood re-entry gate, found by DOGFOODING the live GPUI app:
**`describe_ui` reports the CONTENT column for a focused `editable_text`,
while the window shows the source projection** — the screenshot reads `TODO
existing task` and `describe_ui` at the same instant reports `editable_text
"existing task"`. The whole point of the rebuilt feature is that the
editable surface IS vault syntax, and the MCP surface an agent drives it
through cannot see that surface at all; every check of the projection in
this session had to go through a screenshot and pixel arithmetic. PERCEPTION
primary: no formal invariant is possible over "what the MCP tool chose to
report", and the divergence is between two descriptions of one widget rather
than between two states. Missing piece: `describe_ui` should report the live
editor buffer for a focused `editable_text` (or name both), otherwise
agent-driven verification of any projected surface is blind. Related but
distinct from the rendered-vs-SQL divergences elsewhere in this ledger: here
SQL and the window are both right and the DESCRIPTION is wrong.)

## Missing piece

PERCEPTION: no formal invariant is possible over what a description tool
chooses to report, and the divergence is between two descriptions of one
widget rather than between two states. Missing piece: `describe_ui`
reporting the live editor buffer for a focused `editable_text` (or naming
both), otherwise agent-driven verification of any projected surface is
blind. Distinct from the rendered-vs-SQL divergences elsewhere in this
ledger: here SQL and the window are both right and the DESCRIPTION is wrong.

## Remedy

OPEN — reported, not fixed.
