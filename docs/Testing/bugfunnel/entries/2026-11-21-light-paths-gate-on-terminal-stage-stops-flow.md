---
id: 2026-11-21-light-paths-gate-on-terminal-stage-stops-flow
date: 2026-11-21
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The light-paths gate was declared on the terminal stage `landed`, which flow refuses, so every flow command
  failed to load the config until the line was removed by hand.
---

## Bug

The `[child.light.gates]` line `landed = ["command:light-paths"]` made flow's config loader raise, which stopped every
flow command. Found by flow itself on the first command after the line was added, not by a test.

## Root cause

`scripts/light-paths-gate.py` accepted only `FLOW_TO == "landed"`. Flow refuses a gate on a terminal stage
("a terminal stage is closed by its own command", the loader of the flow skill). The script's tests drove the script
with stub `gh` answers and never loaded a gate line into flow's config.

## Missing piece

No test passed the gate's config line through flow's own loader (`flow.model.load_config`, the entry `flow check` uses),
so the script and the stage it may sit on could disagree unseen.

## Remedy

The script accepts only `FLOW_TO == "verify"`, the last non-terminal move every light card makes.
`scripts/test-light-paths-gate.py` now loads two fixed configs through flow's loader: the gate on `verify` loads, the
gate on `landed` is refused. It also asserts that any other `FLOW_TO` value exits non-zero.
