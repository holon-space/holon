---
name: holon-core
description: Core traits for CRUD, block operations, undo/redo, and fractional indexing
type: reference
source_type: component
source_id: crates/holon-core/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-core (crates/holon-core)

**Purpose**: Foundational trait definitions and data structures shared across the storage and operation layers. No concrete implementations — pure contracts.

### Key Modules & Types

| Module | Key Types |
|--------|-----------|
| `traits` | `CrudOperations`, `BlockOperations`, `TaskOperations`, `MoveOperations`, `RenameOperations` |
| `storage` | `DataSource<T>`, `EditorCursorOperations` |
| `operation_log` | `OperationLogOperations`, `OperationLogEntry`, `OperationResult` |
| `undo` | `UndoStack`, unified undo/redo implementation |
| `fractional_index` | `FractionalIndex` — ordering blocks without reindexing |
| `core` | Core domain logic utilities |

### Trait Overview

- **`CrudOperations<T>`**: `create`, `read`, `update`, `delete`
- **`BlockOperations`**: block-specific ops (insert, move, indent, etc.)
- **`DataSource<T>`**: `get_all()`, `get_by_id()`, `subscribe_changes()` — abstraction over Turso, in-memory, MCP
- **`UndoStack`**: wraps `OperationLogOperations`; supports multi-step undo with collaborative safety

### Related

- Extended by: `holon` (concrete impls), `holon-todoist`, `holon-orgmode`, `holon-mcp-client`
- Used by: `holon-frontend` (operation dispatch), `holon-integration-tests`
