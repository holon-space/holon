---
id: 2026-09-19-editor-cells-fall-back-to-the-on-blur-funnel-unannounced
date: 2026-09-19
gap: ENVIRONMENT
status: OPEN
summary: >-
  Focusing a block in the real GPUI app logged an ERROR saying no editor-cell
  registry is wired, so every keystroke writes through the on-blur funnel rather
  than the per-keystroke CRDT cell, with nothing said to the user.
---

## Bug

Found by the `dogfood-integ` lane clicking blocks in the real GPUI binary on a
copy of Martin's vault. Log
`lane-logs/dogfood-integ-evidence/logs/app-boot1.log`, twice
(20:11:07.251134 and 20:11:32.149118):

```
ERROR holon_gpui::views::editor_view: no editor-cell registry for
block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01: every keystroke will write through
the on-blur funnel instead of the per-keystroke CRDT cell (editable_text not
configured for this ReactiveEngine (BlockCellRegistry not wired))
```

The message is exemplary as a log line: it names the degradation, the mechanism
and the missing wiring. It is also ERROR level, fires per focused block, and the
person typing sees nothing. Under ruling D112.a a first keystroke is a text edit
that creates the Loro node; routing keystrokes through the on-blur funnel
instead changes when and how that node is born.

## Root cause

Not isolated. `BlockCellRegistry` is not wired into the `ReactiveEngine` this
boot resolved. Whether that is specific to this launch (a built binary, real
vault, `--features pbt`) or holds for every GPUI boot was not determined — the
lane stopped at observation.

Establishing which it is, is the first step, and it is cheap: a fresh boot on an
empty sandbox vault either logs the same line or does not.

## Missing piece

The keystone drives text edits through the composed pipeline where the registry
is wired by construction, so the unwired arm is never taken. No rung asserts
that a focused block in the WINDOWED frontend has a per-keystroke cell, which is
the platform-wiring divergence this whole escape class is made of.

## Remedy

Open. Determine the scope (this launch vs every GPUI boot), then either wire the
registry or make the frontend refuse to focus an editor it cannot serve
per-keystroke. A silent downgrade of the write path is a disclosure failure
regardless of which scope it has: the log line is right, its audience is wrong.
