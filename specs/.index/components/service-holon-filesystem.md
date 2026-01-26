---
name: holon-filesystem
description: Filesystem operations, directory watching, and file abstraction layer
type: reference
source_type: component
source_id: crates/holon-filesystem/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-filesystem (crates/holon-filesystem)

**Purpose**: Platform-agnostic file and directory abstractions. Provides directory watching and a DataSource implementation over the local filesystem.

### Key Types

| Type | Role |
|------|------|
| `Filesystem` | Top-level entry point |
| `Directory` | Directory abstraction + `DirectoryDataSource` |
| `File` | File abstraction |
| `DirectoryChangeProvider` | Emits change events when files are created/modified/deleted |
| `FilesystemError` | Unified error type |

### Related

- **holon-orgmode**: uses `OrgFileWatcher` which wraps filesystem watching
- **holon**: wired as a `DataSource` for directory-backed data
