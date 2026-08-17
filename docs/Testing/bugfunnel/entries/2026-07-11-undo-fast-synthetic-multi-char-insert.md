---
id: 2026-07-11-undo-fast-synthetic-multi-char-insert
date: 2026-07-11
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Undo of a fast synthetic multi-char insert removes irregular sub-word chunks
  ('orld', ' w') contra word-boundary grouping — likely zero-delay MCP
  keystrokes defeat the coalescing heuristic; real typing has gaps
source_line: 898
---

## Bug

Undo of a fast synthetic multi-char insert removes irregular sub-word chunks
('orld', ' w') contra word-boundary grouping — likely zero-delay MCP
keystrokes defeat the coalescing heuristic; real typing has gaps

## Missing piece

grouping heuristic untested under zero-inter-key-delay input; needs
human-paced repro before calling it a design bug

## Remedy

OPEN (PARTIAL — repro artifact suspected)
