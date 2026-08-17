---
id: 2026-07-13-martin-live-report-reproduced-dogfood-typed
date: 2026-07-13
gap: ENVIRONMENT
secondary: ORACLE
status: PARTIAL
summary: >-
  P1 (Martin live report REPRODUCED, dogfood #5): typed `[[page]]` link is
  DESTROYED by refocus+blur — marks `Link` entry + `block_links` junction row
  wiped to NULL and the link degrades to plain text in DB AND on disk. Exact
  step isolated over MCP (per-step describe_ui+SQL+junction+disk): typing
  expands fine (per-keystroke commit writes stripped content + marks +
  junction); FIRST blur is clean (no commit — text unchanged); the wipe fires
  on blur AFTER a refocus. Mechanism (two cooperating defects): (a) on editor
  re-seed the visible buffer gets STRIPPED display content while
  change-tracking baseline disagrees (raw-vs-stripped inconsistency; one block
  whose in-session buffer was retained raw `[[Some Page]]` SURVIVED the same
  cycle — 2 of 3 blocks wiped), so blur fires a SPURIOUS `set_field(content)`
  with identical stripped text; (b) `content_marks_followup`
  (operation_dispatcher.rs:568) re-extracts marks from that stripped text →
  zero marks → dispatches `marks=Null` which also deletes the junction row
  (task #66 suspect CONFIRMED as the executor). NOTE: (b) alone is
  keystone-reachable headless — any `set_field(content)` carrying the
  already-stripped text of a marked block wipes its marks
source_line: 972
---

## Bug

P1 (Martin live report REPRODUCED, dogfood #5): typed `[[page]]` link is
DESTROYED by refocus+blur — marks `Link` entry + `block_links` junction row
wiped to NULL and the link degrades to plain text in DB AND on disk. Exact
step isolated over MCP (per-step describe_ui+SQL+junction+disk): typing
expands fine (per-keystroke commit writes stripped content + marks +
junction); FIRST blur is clean (no commit — text unchanged); the wipe fires
on blur AFTER a refocus. Mechanism (two cooperating defects): (a) on editor
re-seed the visible buffer gets STRIPPED display content while
change-tracking baseline disagrees (raw-vs-stripped inconsistency; one block
whose in-session buffer was retained raw `[[Some Page]]` SURVIVED the same
cycle — 2 of 3 blocks wiped), so blur fires a SPURIOUS `set_field(content)`
with identical stripped text; (b) `content_marks_followup`
(operation_dispatcher.rs:568) re-extracts marks from that stripped text →
zero marks → dispatches `marks=Null` which also deletes the junction row
(task #66 suspect CONFIRMED as the executor). NOTE: (b) alone is
keystone-reachable headless — any `set_field(content)` carrying the
already-stripped text of a marked block wipes its marks

## Missing piece

(a) gpui editor re-seed/baseline lifecycle absent from headless wiring; (b)
marks-preservation oracle (links-ruling A/B) not landed when this shipped —
no invariant reddens on marks loss under content-identical set_field

## Remedy

PARTIALLY FIXED same day — defect (b) marks-followup wipe FIXED by the task
#66 stream (see row above; landed on integration); defect (a) spurious
identical-content blur commit after refocus still OPEN (also pollutes undo
stack — see undo row below). Verification protocol for (a): type `[[X]]`
into block, blur, refocus, blur, assert marks + junction + disk `[[X]]`
survive all four steps (evidence: /tmp sandbox session dogfood #5, blocks
test-link-a/b/c)
