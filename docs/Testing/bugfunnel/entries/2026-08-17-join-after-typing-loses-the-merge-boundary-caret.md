---
id: 2026-08-17-join-after-typing-loses-the-merge-boundary-caret
date: 2026-08-17
gap: COVERAGE
status: OPEN
summary: >-
  Backspace-joining a block that was typed into leaves the tracked caret at 0
  instead of the merge boundary.
---

## Bug

Found while authoring keystone coverage for
`2026-08-17-set-field-block-not-found-stale-doc-resolution`. Replay this
sequence over the shipped SqlOnly wiring (`storage=[Turso] sync=[] actors=[]`):

```json
{"transitions": [{"SplitBlock": {"block_id": "block:parent", "position": 2}},
                 {"TypeChars": {"text": "ab"}},
                 {"DeleteBackward": {"count": 3}}]}
```

`inv-editor-caret/mirror` fails: `reference model cursor_byte=2, SUT tracked
caret=0`. The three backspaces delete `b`, then `a`, then join the emptied
tail into its previous sibling; the caret belongs at the merge boundary (2,
the surviving head's length) and the reference model puts it there, but the
SUT's tracked caret stays at 0.

Reproduces identically with the focus-leave funnel rung disabled and with the
`apply_local_edit` re-baseline reverted, so it is independent of both — a
pre-existing divergence this sequence is simply the first to reach.

## Missing piece

COVERAGE: `delete-backward-merges-previous-block-budget` is the only
hand-authored join case and it backspaces into a tail that was never typed
into, where the caret is already 0 and the divergence cannot show. No case
combined `TypeChars` with a `DeleteBackward` that crosses the block boundary.

## Remedy

NOT FIXED. Undetermined whether the defect is in production's caret seeding
(`join_block` arms the merge boundary; `grab_focus_and_seed_caret` /
`adopt_armed_caret_seed` consume it) or is a fidelity gap in
`HeadlessEditorMirror`'s caret tracking — a mirror-only artefact would make
the reference model right and the SUT reading wrong for a reason no user
would ever see. Deciding that is the first step, since it determines whether
this is a user-visible caret bug or a test-double bug.

The transition sequence above is deliberately NOT in `keystone.jsonl`: it
would red the landing gate on this unrelated divergence. Add it there as the
red-first pin when this is fixed.
