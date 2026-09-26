---
id: 2026-09-26-markdown-ingest-never-updates-a-changed-task-marker
date: 2026-09-26
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  On the LogSeq and Obsidian legs, removing or changing a block's task marker
  in the file leaves the stored task_state unchanged: unchecking an Obsidian
  checkbox does not un-complete the task in Holon.
---

## Bug
Found by the verifier of the `?`-question increment (lane `questions`,
`lane-logs/inc1d-verify.md`, section "The markdown / LogSeq / Obsidian leg has
the same bug, unfixed"). It is the markdown twin of
`2026-09-26-org-ingest-never-clears-a-removed-task-keyword`, which was fixed on
the org leg only. The verifier drove both adapters through their real `parse`,
`build_block_params` and `content_differs`:

| File edit | Parsed task_state | Update dispatched |
|-----------|-------------------|-------------------|
| LogSeq `TODO` marker removed | `TODO` to none | no |
| LogSeq `DONE` to plain | `DONE` to none | no |
| Obsidian checkbox removed | `DONE` to none | no |
| Obsidian `- [x]` to `- [ ]` | `DONE` to `TODO` | no |

The store keeps the old task state permanently and contradicts the file. Both
adapters are read-only (`logseq.rs:340`, `obsidian.rs:386`), so write-back
does not put the marker back into the file. The last row is a changed keyword,
not a removed one: a checked-off task that the user unchecks stays done in
Holon.

## Root cause
Two independent causes, both live:

1. `content_differs` in `crates/holon-markdown/src/logseq.rs:327` and
   `crates/holon-markdown/src/obsidian.rs:382` compares `content`, `marks` and
   `tags`, not `task_state`. The marker is stripped from `content`, so a
   marker-only edit dispatches no update at all.
2. `build_block_params` in `crates/holon-markdown/src/params.rs` emits
   `task_state` only when the block has one (line 67) and discards `previous`
   (line 25). The comment there says no key can go stale in the store;
   `task_state` is such a key.

## Missing piece
No keystone transition or hand-authored case edits a task marker in an
existing LogSeq or Obsidian file.

## Remedy
OPEN, routed to another lane. The fix needs both: `content_differs` must
compare the task state, and the params builder must clear `task_state` and
`task_state_category` when `previous` had one and the file no longer does, as
`crates/holon-orgmode/src/block_params.rs` does. Red first with a case per
adapter for a removed marker and for `- [x]` to `- [ ]`.
