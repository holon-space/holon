---
name: holon-orgmode
description: Org-mode disk I/O, bidirectional sync controller, file watching, DI wiring
type: reference
source_type: component
source_id: crates/holon-orgmode/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-orgmode (crates/holon-orgmode)

**Purpose**: Org-mode sync: watches files on disk, parses changes, writes back from SQL, and prevents echo loops. Single source of truth for org I/O.

### Key Modules & Types

| Module | Key Types |
|--------|-----------|
| `org_sync_controller` | `OrgSyncController` — main bidirectional sync state machine |
| `orgmode_sync_provider` | `OrgModeSyncProvider` — stream-based sync provider (walks dirs) |
| `orgmode_event_adapter` | `OrgModeEventAdapter` — translates file events to block operations |
| `file_watcher` | `OrgFileWatcher` — native filesystem watcher |
| `file_io` | Source block insertion/update utilities |
| `traits` | `BlockReader`, `DocumentManager`, `OperationProvider` — storage-agnostic traits |
| `di` | DI wiring (conditional on `di` feature) |
| `block_params` | Block parameter building |

### Sync Architecture

- **Single renderer**: `OrgRenderer` is the ONLY path for producing org text from blocks
- **Echo suppression**: `last_projection: HashMap<PathBuf, String>` — compares disk vs last-written; no timing windows
- **`blocks_differ()`**: compares content, parent_id, content_type, source_language, task_state, priority, tags, scheduled, deadline, org_properties
- Source blocks render BEFORE text children (org format requirement)
- `OrgModeSyncProvider` only walks dirs and emits events — no parsing/writing

### Document Identity

- Documents have two URIs: file-path-based (`holon-doc://file.org`) and UUID-based (`holon-doc://{uuid}`)
- `LoroDocumentStore.register_alias(uuid, path)` maps UUID → canonical file path
- OrgSyncController rewrites root block parent_ids from file-based to UUID-based URIs
- When upserting blocks: if both old/new parent_ids are document URIs, update field only (don't move in tree)

### Trait Implementations (DI)

- `CacheBlockReader` wraps `QueryableCache<Block>` — uses `DataSource::get_all()` + in-memory filtering
- `DocumentManagerAdapter`, `LoroAliasRegistrar` implement traits in `di.rs`

### Related

- **holon-org-format**: pure parsing/rendering (no I/O); orgmode wraps it
- **holon**: `sync` module wires OrgSyncController into backend
