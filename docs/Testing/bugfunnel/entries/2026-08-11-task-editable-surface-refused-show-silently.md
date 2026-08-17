---
id: 2026-08-11-task-editable-surface-refused-show-silently
date: 2026-08-11
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A task the editable surface REFUSED to show was silently deleted by the next
  keystroke.
source_line: 740
---

## Bug

(task #78 arm-(d) lane, found by ADVERSARIAL VERIFICATION — a fresh-context
verifier refuted the lane's Inc 3 safety claim and reproduced it through the
real dispatcher) **A task the editable surface REFUSED to show was silently
deleted by the next keystroke.** The seed refusal is judged under the
DOCUMENT's vocabulary; the commit router was judged by the vocabulary-FREE
`could_converge`. A refused buffer whose stored content merely starts with
an uppercase token reached the source channel, where the store correctly
found no DECLARED keyword and wrote `task_state = ""` — with no warning,
because the only WARN fires at the seed. Repro: `#+TODO: NEXT WAITING \ |
DONE`; block `content="ASAP call Bob"`, `task_state="TODO"`; type `!`; task
gone. Reachable by marking blocks TODO under the defaults and THEN adding a
`#+TODO:` line.

## Root cause

task #78 arm-(d) lane, found by ADVERSARIAL VERIFICATION — a fresh-context
verifier refuted the lane's own Inc 3 safety claim and reproduced the defect
end to end through the real dispatcher (`lane78d-verify.md`, probe
`v-probe.txt`): **a task the editable surface REFUSED to show was silently
deleted by the next keystroke.** The seed refusal is judged under the
DOCUMENT's vocabulary (`source_projection`), but the commit router was
judged by `could_converge`, which is vocabulary-FREE — any 2..32
ASCII-uppercase token. So a refused buffer whose stored CONTENT merely
starts with such a token was admitted to the source channel, where
`run_set_source_text` correctly found no DECLARED keyword and wrote
`task_state = ""`. Repro: page declares `#+TODO: NEXT WAITING | DONE`; block
`content="ASAP call Bob"`, `task_state="TODO"` (marked before the doc
declared its own vocabulary); the seed refuses and WARNs; the user types
`!`; the task is gone, with NO warning — the seed's WARN had already fired
and the commit path never knew. Reachable by an ordinary authoring sequence:
mark blocks TODO under the defaults, then add a `#+TODO:` line to the page —
every pre-existing task is now refused, and every one whose text starts with
`API`/`PR`/`ASAP`/… loses its task on the next keystroke. ORACLE primary,
and the sharpest form of it: the lane HAD a rung for both halves —
`a_task_state_the_document_does_not_declare_is_not_projected` pins the seed,
`deleting_the_keyword_still_takes_the_source_channel` pins the router — and
both were green, because no assertion anywhere related the two judgments to
each other. The missing piece is not a draw, it is a PROPERTY: seed and
commit must derive from ONE vocabulary-aware judgment. Secondary COVERAGE:
no fixture ever put a block in the refused class at all (it needs a document
that declares a vocabulary EXCLUDING a keyword its own blocks already
carry). FIXED 2026-08-11 in the same lane: the seed outcome is now carried
as data — `editor_source::Surface::{Untasked, Projected, Refused}` — and
`EditorViewModel::commits_as_source` routes on it, pinning a Refused surface
to the content channel for the whole session, so the task is neither
editable NOR removable through a surface that could not show it. Red-first
at the exact counterexample
(`a_refused_surface_never_commits_through_the_source_channel`, red `left:
String("source_text") right: String("content")`), plus a session-pin arm,
plus two cannot-pass-by-refusing-everything locks
(`a_projected_surface_still_commits_as_source` including the demotion edit).
The ADJACENT cell — an UNTASKED block whose text has the shape of an
undeclared keyword — was already safe (the store skips the task-state
constituent when there is nothing to clear) and is now locked by
`a_source_write_that_declares_nothing_leaves_a_plain_block_plain`.)

## Missing piece

ORACLE, in its sharpest form: rungs existed for BOTH halves (seed refusal,
router) and both were green — nothing anywhere related the two judgments to
each other. The missing piece is a PROPERTY, not a draw: seed and commit
must derive from one vocabulary-aware judgment. COVERAGE secondary: no
fixture ever produced a refused block, which needs a document declaring a
vocabulary that EXCLUDES a keyword its own blocks already carry.

## Remedy

FIXED 2026-08-11 in the same lane. The seed outcome is carried as data
(`editor_source::Surface::{Untasked, Projected, Refused}`) and
`EditorViewModel::commits_as_source` routes on it: a Refused surface is
pinned to the content channel for the whole session, so a task the surface
could not SHOW it also cannot REMOVE. Red-first at the verifier's exact
counterexample
(`a_refused_surface_never_commits_through_the_source_channel`, red `left:
String("source_text") right: String("content")`) plus a session-pin arm;
`a_projected_surface_still_commits_as_source` (including the demoting edit)
stops the guard passing by refusing everything. Adjacent cell — an untasked
block whose text has the shape of an undeclared keyword — was already safe
(the store skips the task-state constituent when there is nothing to clear)
and is now locked by
`a_source_write_that_declares_nothing_leaves_a_plain_block_plain`.
