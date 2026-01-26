---
name: holon-todoist
description: Todoist integration with task sync, converters, fake clients for testing
type: reference
source_type: component
source_id: crates/holon-todoist/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-todoist (crates/holon-todoist)

**Purpose**: Bidirectional Todoist task sync. Provides a real HTTP client and an in-memory fake for testing.

### Key Modules & Types

| Module | Key Types |
|--------|-----------|
| `client` / `api_client` | `TodoistClient` (HTTP), `TodoistApiClient` trait |
| `fake` / `fake_client` | In-memory fake implementations for tests |
| `todoist_sync_provider` | `TodoistSyncProvider` — stream-based sync |
| `todoist_datasource` | `TodoistDataSource` — stream-based data source |
| `todoist_event_adapter` | `TodoistEventAdapter` — translates Todoist webhooks to block ops |
| `models` | `TodoistTask`, `TodoistProject` |
| `converters` | Todoist ↔ Block type conversions |
| `queries` | Query builders |
| `di` | `TodoistModule`, `TodoistConfig`, `TodoistInjectorExt` |

### Trait

`TodoistMoveOperations` — move task between projects/sections

### Related

- **holon-core**: implements `DataSource<TodoistTask>` and operation traits
- **holon** DI: `TodoistModule` wired into `CoreInfraModule`
