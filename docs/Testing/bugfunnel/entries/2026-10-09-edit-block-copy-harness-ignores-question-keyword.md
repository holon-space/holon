---
id: 2026-10-09-edit-block-copy-harness-ignores-question-keyword
date: 2026-10-09
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The keystone harness `edit_block_copy` did not recognise the `?` question
  keyword, so Clear/TODO/DONE on a `* ? Q :decision:` copy kept the `?` while
  the reference model replaced it.
---

## Bug
Keystone signature `[inv-task-state-matches-ref] block:kd-1: expected
task_state=None (reference), actual task_state=Some("?")`, triaged in the kd1
lane (lane-logs/kd1/, probe3.jsonl). No product defect.

## Root cause
`edit_block_copy` (crates/holon-integration-tests/src/pbt/frontend_slice/components.rs)
built the keyword list passed to `holon_orgmode::subtree::set_headline_keyword`
from `TASK_STATE_CYCLE` only. That list lacks
`holon_org_format::task_keyword::QUESTION_KEYWORD`, so the `?` stayed in the
title: Clear rewrote nothing and TODO wrote `* TODO ? Q`. The reference
(`apply_to_ref` in edit_block_copy.rs) correctly replaces the keyword.

## Missing piece
The harness keyword list did not include every production keyword.

## Remedy
The list now chains `QUESTION_KEYWORD`. Pinned by the hand-authored rows
`a-decision-headline-copy-edited-{clear,todo,done}-keeps-the-reference` in
crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl.
