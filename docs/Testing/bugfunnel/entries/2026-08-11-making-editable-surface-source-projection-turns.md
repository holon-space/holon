---
id: 2026-08-11-making-editable-surface-source-projection-turns
date: 2026-08-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Making the editable surface a SOURCE PROJECTION turns the #64
  content-channel contract into a DOUBLING bug: the surface gains a keyword
  per focus cycle.
source_line: 742
---

## Bug

(task #78 arm-(d) implementation lane, found by agent investigation while
writing the increment plan; no automated test produced it) **Making the
editable surface a SOURCE PROJECTION turns the #64 content-channel contract
into a DOUBLING bug: the surface gains a keyword per focus cycle.** The plan
had the editor commit its raw buffer as `set_field("content")` on the
reasoning that the store's convergence IS the parse; that holds only while
the block is untasked, because `keyword_convergence` short-circuits on
`stored_task_keyword(id).is_some()`. On a task, the raw buffer `TODO milk`
therefore lands INSIDE the content of a block already carrying `TODO`, and
the next focus seeds `TODO TODO milk`. The witness needed no new code:
`an_already_tasked_block_is_refused_and_still_commits` already asserted
`content == "DONE buy milk"` beside `task_state == "TODO"` — correct under
#64, catastrophic under a projected surface.

## Root cause

task #78 arm-(d) implementation lane, found by AGENT INVESTIGATION while
writing the increment plan — no automated test produced it, and the shape it
predicts was ALREADY pinned by the suite as correct behaviour: **making the
editable surface a source projection turns the #64 content-channel contract
into a DOUBLING bug — the surface gains a keyword per focus cycle.** The
plan said the editor would commit its raw buffer as `set_field("content")`
because "the store's convergence IS the parse". That holds only while the
block is untasked: `OperationEngine::keyword_convergence` short-circuits on
`stored_task_keyword(id).is_some()`, so once the block is a task the raw
buffer `TODO milk` lands INSIDE the content of a block already carrying
`TODO`. Not demotion — doubling, and it compounds: focus seeds `TODO TODO
milk` next time. The witness needed no new code, only a new reading of an
existing test — `an_already_tasked_block_is_refused_and_still_commits`
asserted exactly `content == "DONE buy milk"` beside `task_state == "TODO"`.
Primary COVERAGE: no rung at any layer ever re-committed a block's OWN
editable surface unchanged (focus a task, type one character, look), because
before this lane the surface never carried the keyword and the gesture did
not exist to draw. Secondary ORACLE, and load-bearing: the keystone
reference modelled the editor buffer as the CONTENT column, so even the
missing draw would have agreed with prod's wrong answer — the ref had no
representation of a surface distinct from the column it projects. FIXED
2026-08-11 in this lane by the d2 sub-ruling: the editor commits
`set_field("source_text")`, a field with different semantics available to
every caller, and the engine parses it under the document's vocabulary and
writes `content` + `task_state` as one composite undo entry. Regression
LOCKS, red-first in two cuts (the first was vacuous and is recorded as such,
`lane-logs/task78/inc1-d2-red.txt` / `inc1-d2-red-doubling.txt`):
`recommitting_the_source_projection_does_not_double_the_keyword` reds in the
doubling shape verbatim (`left: Some("TODO milk") right: Some("milk")`) when
the source commit is routed to the content channel, beside
`the_content_channel_never_re_derives_the_task_state` and
`an_agent_content_write_on_a_tasked_block_is_not_re_parsed`, which pin that
the #64 contract is UNCHANGED — the two channels convict each other. The
ORACLE half is closed too: the reference now projects `editor_surface_text`
at every editor seed and routes its commits through the same
`holon_org_format::source_channel_commit` rule prod uses.)

## Missing piece

COVERAGE: no rung ever re-committed a block's OWN editable surface unchanged
(focus a task, type one character), because before this lane the surface
never carried the keyword and the gesture did not exist to draw. ORACLE
(load-bearing): the keystone reference modelled the editor buffer as the
CONTENT column, so even that draw would have agreed with prod — the ref had
no representation of a surface distinct from the column it projects.

## Remedy

FIXED 2026-08-11 by the d2 sub-ruling: the editor commits
`set_field("source_text")`, a field with different semantics open to every
caller, and the engine parses it under the owning document's vocabulary and
writes `content` + `task_state` as ONE composite undo entry. Red-first in
two cuts, the first VACUOUS and recorded as such
(`lane-logs/task78/inc1-d2-red.txt`, `inc1-d2-red-doubling.txt`):
`recommitting_the_source_projection_does_not_double_the_keyword` reds in the
doubling shape verbatim when the source commit is routed to the content
channel. The #64 contract is pinned UNCHANGED beside it by
`the_content_channel_never_re_derives_the_task_state` and
`an_agent_content_write_on_a_tasked_block_is_not_re_parsed`. ORACLE half
closed: the reference projects `editor_surface_text` at every editor seed
and routes commits through the same
`holon_org_format::source_channel_commit` rule prod uses, and the
hand-authored fixture `task64-promotion-loro-arm` flipped RED→GREEN with the
ref model SIMPLIFIED (its promotion branch deleted), not accommodated.
