---
id: 2026-10-02-a-second-backspace-joins-from-a-block-the-first-join-consumed
date: 2026-10-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Two fast Backspaces at the start of a block in the GPUI editor: the second sends
  join_block from the block the first join consumed, so it fails with "Block not found"
  and the second character is not deleted.
---

## Bug
Found by the headless keystone after its editor twin was made to dispatch
structural keys fire-and-forget, as GPUI does (admission Inc 0a). NOT observed
in the live app; the defect is inferred from the code path GPUI shares with the
twin and from the keystone red below.

Hand-authored row `split-then-two-backspaces-caret-follows-second-join`:
`inv-no-observed-errors` reports `dispatch_intent_chain: block.join_block
failed — aborting remaining intents: ... Operation 'join_block' on entity
'block' failed: Block not found`, and `inv-editor-text/mirror` shows `"parent"`
where the reference holds `"prent"`. Red log:
`lane-logs/adm0a3-ha-after-4.log:2089`.

## Root cause
- Backspace at caret 0 dispatches `join_block` for the editor's own row through
  `dispatch_structural_as_commit_point`
  (`frontends/gpui/src/views/editor_view.rs:1607-1619`), a detached chain
  (`crates/holon-frontend/src/reactive.rs`, `dispatch_intent_chain`).
- The first join consumes the block, but its editor and `InputState` stay
  focused until the row disappears from the render. A second Backspace in that
  window sends a second `join_block` for the consumed block, which the store
  refuses (`Block not found`). Nothing admits the second join against the
  state the first one left.

## Missing piece
The keystone's headless editor awaited each structural dispatch, so focus had
always moved to the merge target before the next key; the window could not
occur in the test.

## Remedy
OPEN. Registered as known red `join-then-stale-second-join`
(`docs/Testing/KeystoneKnownReds.md`); the row is in the `just hand-authored`
default skip list. Owner: admission Inc 2 (join-then-stale).
