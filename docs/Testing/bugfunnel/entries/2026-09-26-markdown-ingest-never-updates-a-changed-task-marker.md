---
id: 2026-09-26-markdown-ingest-never-updates-a-changed-task-marker
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
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
existing LogSeq or Obsidian file. The keystone cannot drive Markdown vaults at
all: the markdown adapters are not in the production format registry
(`crates/holon-app/src/wiring.rs`, D56.a: both claim `md`, and the registry
refuses that until a vault-flavor discriminator exists). The bug is latent for
the same reason.

## Remedy
FIXED. The markdown `content_differs` (LogSeq and Obsidian) now
compares `task_state`, so a changed or removed marker produces an update op.
The markdown `build_block_params` now reads `previous` and emits
`Value::REMOVED` for `task_state` and `task_state_category` when the file
dropped the marker, the same as the org leg's eraser. Pinned by
`crates/holon-markdown/tests/task_marker_reingest.rs` (5 bug shapes + 3
negatives). Teeth: removing either fix turns the matching tests red.
Coverage gap: the keystone cannot drive Markdown vaults, because the adapters
are not in the production registry (D56.a). The same gate misses other
planning fields and properties:
`2026-09-26-markdown-ingest-never-updates-planning-or-property-changes`.
