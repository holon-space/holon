---
name: loro
description: CRDT framework powering org-mode bidirectional sync and conflict resolution
type: reference
source_type: url
source_id: https://docs.rs/loro/latest/loro/
fetch_timestamp: 2026-04-23
---

## Loro (v1.0)

**Purpose**: High-performance CRDT framework for conflict-free distributed state management with undo/redo and time-travel.

### Key APIs

| Type | Role |
|------|------|
| `LoroDoc` | Root container; entry point for all operations |
| `LoroText` | Plaintext / rich-text with cursor tracking |
| `LoroMap` | CRDT key-value container |
| `LoroList` | CRDT ordered array |
| `LoroTree` | Hierarchical structure (used for block trees) |
| `LoroMovableList` | Drag-drop enabled list |
| `LoroCounter` | Distributed counter |
| `UndoManager` | User-specific undo preserving remote edits |
| `VersionVector` | Tracks document evolution for incremental sync |
| `.checkout(frontiers)` | Read-only navigation to historical state |
| `.revert_to(frontiers)` | Mutate history (rollback) |

### Import/Export Modes

- **All updates** — full sync blob
- **Snapshot** — compressed current state
- **Incremental** — changes since last version vector

### Integration in Holon

- `holon-orgmode`: `OrgSyncController` uses LoroDoc to track block content changes, computes diffs against last-written version to avoid echo loops
- `lorotree-spike` (experiments): validated global LoroTree replacing per-file LoroDoc approach
- `holon` storage layer: Loro reconcile writes to Turso; stale UPDATE guard uses WHERE clause to prevent overwriting concurrent edits
- Peer ID management critical for multi-device scenarios

### Keywords
CRDT, sync, conflict-free, org-mode, LoroDoc, LoroMap, LoroTree, bidirectional-sync
