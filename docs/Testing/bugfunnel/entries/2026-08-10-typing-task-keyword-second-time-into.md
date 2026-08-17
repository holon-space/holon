---
id: 2026-08-10-typing-task-keyword-second-time-into
date: 2026-08-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Typing a task keyword a SECOND time into a block silently DELETED the typed
  text. Live promotion strips the keyword from the editor's buffer BEFORE the
  write is confirmed, and the trigger ran only two of the engine's three
  guards — it could not see the block's `task_state`. On an already-tasked
  block it proposed a promotion, the engine correctly refused and committed
  `TODO x` verbatim, and the stripped buffer overwrote that commit on the next
  keystroke: stored content `x`.
source_line: 746
---

## Bug

(task #64 Inc 3 lane, found by agent exploration — a fresh-context verifier
hand-authored six promotion edge shapes into a probe sidecar) Typing a task
keyword a SECOND time into a block silently DELETED the typed text. Live
promotion strips the keyword from the editor's buffer BEFORE the write is
confirmed, and the trigger ran only two of the engine's three guards — it
could not see the block's `task_state`. On an already-tasked block it
proposed a promotion, the engine correctly refused and committed `TODO x`
verbatim, and the stripped buffer overwrote that commit on the next
keystroke: stored content `x`.

## Root cause

task #64 Inc 3 lane, found by AGENT EXPLORATION — a fresh-context verifier
hand-authored six promotion edge shapes into a probe sidecar; no committed
test and no keystone run produced it: **typing a task keyword a SECOND time
into a block silently DELETED the typed text.** Live promotion makes the
editor view model strip the keyword out of its own buffer BEFORE the write
is confirmed, and its trigger ran only two of the engine's three guards — it
could not see the block's `task_state`. So on an already-tasked block it
proposed a promotion, the engine correctly REFUSED and committed the typed
text verbatim (`TODO x`), and the view model's stripped buffer then
overwrote that commit on the very next keystroke: stored content `x`, the
keyword gone. Reachable from ordinary typing (`TypeChars "TODO "` then
`TypeChars "TODO x"` on one focused block) and from the unconditional
keyword generator arm, in BOTH storage arms. Classified COVERAGE, in its
sharpest form: the ORACLE was fully adequate — once the shape existed, SEVEN
invariants went red (`inv-editor-text/mirror`, `inv-editor-caret/mirror`,
`inv-blocks-match-ref/{block_raw,org,matview}`,
`inv-block-content/{block_raw,sql}`) — and the code path runs in the
keystone's own wiring, so it is neither ORACLE nor ENVIRONMENT; the escape
is pure generation, because both committed fixtures were a single
`TypeChars` into an empty untasked block and no draw ever typed a SECOND
keystroke after a promotion. FIXED 2026-08-10 in the same rev, in TWO rounds
— the first attempt is recorded because its failure is the lesson. Round 1
gave `EditorViewModel` a cached `task_keyword`, seeded at mount and advanced
by its own promotions; a second verification round REFUTED it with two
counterexamples the cache could not survive: (a) `ToggleState` through the
non-editor widget AFTER the editor is open makes the cached value stale and
the identical seven-invariant text loss returns, (b) GPUI nulls the per-row
data handle for cell-attached (Loro) editors, so in the shipped-Loro arm the
cache was never seeded at all. Round 2 deleted the cache: the trigger now
asks `EditorViewModel::attempts_promotion` (pure, no read) and, only for a
keyword-headed keystroke, the caller reads the block's CURRENT task keyword
and passes it in as `TaskKeywordAtKeystroke` —
`QueryEngine::block_task_state_by_id` headlessly, the row projection's
`task_state` in GPUI (read from a handle taken BEFORE the cell-attached
nulling). A THIRD verification round then showed that freshness alone still
cannot close the class: the read and the engine's guard are not one
transaction (dispatch was fire-and-forget), so any concurrent writer of
`task_state` — a peer, an agent, a rule — landing between them turns an
accepted proposal into a refusal with a perfectly fresh read, and the
optimistically stripped buffer overwrites the engine's verbatim commit
exactly as before. Round 3 therefore made the optimism RECOVERABLE instead
of arguing it is safe: the promotion dispatch (promotion only; ordinary
content commits stay fire-and-forget) goes through the new
`BuilderServices::dispatch_intent_awaiting_result`, and the compound's
existing refusal payload — `outcome:"refused"` plus the text it committed
verbatim, which both callers previously discarded — drives
`EditorViewModel::restore_refused_promotion`: the keyword goes back into the
buffer and the visible field, the caret with it, and any keystroke that
raced the verdict is kept after it (with a follow-up commit when the race
made the restored text differ from what the engine stored). A dispatch that
fails outright restores AND commits, so no keystroke is lost to a failed op.
The class is CLOSED, not narrowed: a refusal is now harmless whatever caused
it, so neither the row-projection lag nor the read/guard interval is a
data-loss window any more, and no freshness argument is load-bearing
anywhere. Gap closed by NINE hand-authored keystone fixtures
(`task64-second-keyword-draw-{sqlonly,loro}`,
`task64-promotion-{double-space,second-keyword-inline,prepend-midtext}`,
`task64-already-tasked-retype`,
`task64-toggle-under-open-editor-{sqlonly,loro}`,
`task64-mount-on-existing-task`; corpus 44→53) plus 10 windowed GPUI tests
including two on a CELL-ATTACHED editor and two refusal-recovery rungs, all
mutation-proven (a stale-at-mount mutation reds the toggle pair with the
verifier's exact signature; nulling the cell arm's row handle reds the
cell-arm test; discarding the verdict reds both recovery tests with `left:
"milk" right: "TODO milk"`). CO-DISCOVERED and fixed alongside, NOT
separately counted because the fixture found it rather than a human: the org
parser re-stripped a keyword that `title_raw()` had already excluded, so a
task whose text legitimately begins with a keyword (`* TODO TODO x`) lost
that word on every read.)

## Missing piece

No draw ever typed a SECOND keystroke after a promotion: both committed
fixtures were one `TypeChars` into an empty untasked block. The oracle was
adequate (7 invariants red once the shape existed) and the path runs in the
keystone's own wiring, so neither ORACLE nor ENVIRONMENT.

## Remedy

FIXED 2026-08-10, same rev, in two rounds. Round 1 (a cached `task_keyword`
seeded at mount) was REFUTED by a second verification: a `ToggleState` under
an open editor makes the cache stale, and GPUI never seeded it at all for
cell-attached (Loro) editors. Round 2 deleted the cache —
`attempts_promotion` (pure) gates a read of the CURRENT keyword, passed in
as `TaskKeywordAtKeystroke` (`QueryEngine::block_task_state_by_id`
headlessly, the row projection in GPUI from a handle taken before the
cell-attached nulling). One source, read where used. Nine keystone fixtures
(incl. `task64-toggle-under-open-editor-{sqlonly,loro}` +
`task64-mount-on-existing-task`; corpus 44→53) and 10 windowed tests incl.
two cell-attached and two refusal-recovery rungs, all mutation-proven. Round
3 then closed the class rather than narrowing it: the trigger's read is not
transactional with the dispatch, so a concurrent `task_state` writer can
force a refusal however fresh the read is — the promotion dispatch is now
AWAITED (`dispatch_intent_awaiting_result`) and a refusal restores the
keyword to the buffer, the visible field and the caret from the compound's
own verbatim payload, keeping any keystroke that raced the verdict. No
freshness argument is load-bearing and no data-loss window remains.
Co-fixed, not separately counted (a fixture found it, not a human): the org
parser re-stripped a keyword `title_raw()` had already excluded, so `* TODO
TODO x` lost a word on every read.
