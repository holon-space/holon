---
id: 2026-08-07-rapid-enter-silently-loses-block-splits
date: 2026-08-07
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Rapid Enter silently loses block splits
source_line: 1163
---

## Bug

(overnight dogfood-explorer, same session) **Rapid Enter silently loses
block splits**, because each split is behind the ~800ms latency above.
Controlled repro: caret placed mid-line in a 39-char block, then 5 Enter
presses at ~60ms spacing — exactly ONE split landed; the other four produced
no block, no banner, and NO LOG LINE AT ALL. A second variant of the same
race does log, and is still silent in the UI: `dispatch_intent_chain:
block.split_block failed — aborting remaining intents: dispatch_intent_sync:
block.split_block failed: Operation 'split_block' on entity 'block' failed:
Split position 21 exceeds content length 19` (and, from the same burst,
`Split position 42 exceeds content length 36`) — a stale caret offset
dispatched against content that the in-flight split had already shortened.
The user-visible result is silent data merging: three lines typed as
separate blocks became one block, `nested a ~b~ c edgebracket Links
reftilde-at-end x`.

## Root cause

secondary ORACLE: overnight dogfood — rapid Enter SILENTLY loses block
splits while a previous `split_block` is in flight behind the ~800ms latency
above. Controlled repro: caret mid-line, 5 Enter presses at ~60ms spacing
produced ONE split; the other four produced no block, no banner, and no log
line at all. A second variant DOES log and is still silent to the user:
`dispatch_intent_chain: block.split_block failed — aborting remaining
intents: … Split position 21 exceeds content length 19` (and `position 42
exceeds content length 36`), i.e. a stale caret offset dispatched against
content the in-flight split had already changed. User-visible effect: text
the user separated with Enter is silently concatenated into one block)

## Missing piece

The keystone settles to quiescence BETWEEN transitions, so it structurally
never issues a second interaction while a projection is in flight — the
entire race is outside what it can generate in time, not in shape. Secondary
ORACLE: no invariant states "every committed Enter yields exactly one
split", which is the assertion that would catch both variants. Missing piece
= a transition-pair generator that fires interaction N+1 before N's
projection lands (an explicit no-settle rung), plus that conservation
invariant. Fail-loud violation to fix regardless: the erroring variant
aborts the intent chain with no user-visible banner.

## Remedy

OPEN 2026-08-07 — diagnosis only.
