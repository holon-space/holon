---
id: 2026-08-11-editable-surface-classified-under-fabricated-vocabulary
date: 2026-08-11
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  The editable surface was classified under a FABRICATED vocabulary during the
  mount window, and a keystroke there silently deleted the task.
source_line: 739
---

## Bug

(task #78 arm-(d) lane, found by ADVERSARIAL VERIFICATION, delta round — the
verifier refuted the round-5 report's disclosed-residual safety claim and
reproduced both arms) **The editable surface was classified under a
FABRICATED vocabulary during the mount window, and a keystroke there
silently deleted the task.** The `#+TODO:` vocabulary needs a page-ancestor
DB round trip, but `TaskKeywordVocabulary::default()` DECLARES `TODO` — so
the first `project_authority` of every mount judged against a guess. A block
the real vocabulary would REFUSE was projected instead, pinned `Projected`,
routed to the source channel, and had `task_state` cleared with the keyword
folded into its content; an unclassified block fell to the vocabulary-FREE
shape rule and reached the same channel.

## Root cause

task #78 arm-(d) lane, found by ADVERSARIAL VERIFICATION (delta round) — the
verifier refuted the round-5 report's own disclosed-residual SAFETY CLAIM
and reproduced both arms (`lane78d-verify.md`, `v5-probe-window*.txt`):
**the editable surface was classified under a FABRICATED vocabulary during
the mount window, and a keystroke there silently deleted the task.** The
`#+TODO:` vocabulary needs a page-ancestor DB round trip, but
`TaskKeywordVocabulary` has a `Default` that DECLARES `TODO` — so the first
`project_authority` of every mount classified against a guess. Arm A: a
block the real vocabulary would REFUSE (`content="ASAP call Bob"`,
`task_state="TODO"` under `#+TODO: NEXT WAITING | DONE`) instead projected
to `TODO ASAP call Bob`, was pinned `Projected`, and routed to the source
channel; the store then found no declared keyword and wrote `task_state=""`
with the keyword folded into the content. Arm B: an unclassified block fell
to the Untasked arm's vocabulary-FREE shape rule and reached the same
channel. Same silent-task-loss class as the round-5 defect, narrower reach
(needs a keystroke inside the resolution window). ORACLE primary, and it is
a REPRESENTABILITY failure, not a missing draw: "vocabulary resolved to the
defaults" and "vocabulary not resolved" were the SAME VALUE, so no assertion
at any layer could have told them apart — and the round-5 report asserted
safety for the window from a path it had never related to the property, the
identical mistake its own "What I got wrong" section describes. COVERAGE
secondary: no rung typed inside the window, and none could, because nothing
made the window observable. FIXED 2026-08-11 by DELETING the fabrication
path rather than narrowing it: `EditorViewModel::vocabulary` is now
`Option`, unresolved is the explicit state
`editor_source::Surface::Pending`, and its commit routing is the SAFE
channel (content — cannot touch a task state) exactly as `Refused` is;
`vocabulary_for_block` returns `Option` so a wiring with no query capability
answers "cannot resolve" instead of "declares nothing"; the headless mirror
now resolves the REAL vocabulary on EVERY seed (the previous read-skip for
untasked blocks was the same bug in cheaper clothing — it classified
`Untasked` under a guess and that arm routes on the vocabulary-free rule).
The seed re-projects when the real vocabulary lands, so the window CLOSES
rather than pinning editors to the content channel forever. RED-FIRST and
DETERMINISTIC — no race:
`an_unresolved_vocabulary_never_classifies_the_surface` encodes both
verifier arms as "nobody called `set_task_vocabulary` yet", which IS the
mount window (red: `left: "TODO ASAP call Bob" right: "ASAP call Bob"`);
`the_resolved_vocabulary_classifies_and_reopens_the_source_channel` is the
anti-overcorrection lock, proving Refused/Projected/promotion-by-typing all
work once the vocabulary arrives. The windowed GPUI rung independently
CONFIRMS the gating is real: all seven projection tests went red the moment
`TestServices` had no resolvable vocabulary, and green again once it answers
honestly (`DeclaresNothingQueryEngine`). Tree-wide audit of every
`TaskKeywordVocabulary::default()` in the round-6 report; `Default` is KEPT
because its remaining uses are real answers — the org parser's documented
"declares none ⇒ defaults" precedence inside `for_document`/`from_declared`,
a `cycle_ring` comparison, and tests that state a document's vocabulary
explicitly.)

## Missing piece

ORACLE, as a REPRESENTABILITY failure rather than a missing draw: "resolved
to the defaults" and "not resolved" were the SAME VALUE, so no assertion at
any layer could distinguish them — and the report again asserted safety for
a path it had not related to the property. COVERAGE secondary: no rung typed
inside the window, and none could, because nothing made the window
observable.

## Remedy

FIXED 2026-08-11 by DELETING the fabrication path, not narrowing it:
`EditorViewModel::vocabulary` is `Option`, unresolved is the explicit
`editor_source::Surface::Pending` whose routing is the safe content channel
(as `Refused` is), `vocabulary_for_block` returns `Option` so "no query
capability" no longer reads as "declares nothing", and the headless mirror
resolves the REAL vocabulary on every seed (its read-skip for untasked
blocks was the same bug in cheaper clothing). The seed re-projects when the
vocabulary lands, so the window closes. Red-first and DETERMINISTIC — no
race: `an_unresolved_vocabulary_never_classifies_the_surface` encodes both
arms as "nobody called `set_task_vocabulary` yet", which IS the window (red
`left: "TODO ASAP call Bob"`), with
`the_resolved_vocabulary_classifies_and_reopens_the_source_channel` as the
anti-overcorrection lock. The windowed GPUI rung confirms the gating is
real: all 7 projection tests reddened when the fixture had no resolvable
vocabulary and greened once it answers honestly. `Default` is KEPT after a
tree-wide audit — its remaining uses are real answers, not guesses.
