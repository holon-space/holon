---
title: holon-orgmode crate (org-mode sync)
type: entity
tags: [crate, orgmode, sync, parser, renderer]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-orgmode/src/lib.rs
  - crates/holon-orgmode/src/org_sync_controller.rs
  - crates/holon-orgmode/src/parser.rs
  - crates/holon-orgmode/src/org_renderer.rs
  - crates/holon-orgmode/src/traits.rs
  - crates/holon-orgmode/src/file_watcher.rs
  - crates/holon-orgmode/src/models.rs
---

# holon-orgmode crate

Bidirectional sync between org-mode files on disk and the Holon block store. Handles parsing, rendering, file watching, and echo suppression.

## OrgSyncController

`crates/holon-orgmode/src/org_sync_controller.rs` — the unified sync controller. Runs on a single tokio task, serializing all file and block change events.

```rust
pub struct OrgSyncController {
    last_projection: HashMap<CanonicalPath, String>,
    block_reader: Arc<dyn BlockReader>,
    command_bus: Arc<dyn OperationProvider>,
    doc_manager: Arc<dyn DocumentManager>,
    root_dir: PathBuf,
    alias_registrar: Option<Arc<dyn AliasRegistrar>>,
    post_org_write_hook: Option<String>,
}
```

### Echo Suppression

The `last_projection` map tracks what the controller last wrote to each file. On a file change event:
1. Read disk content
2. Compare against `last_projection[file]`
3. If equal → this is our own write, suppress
4. If different → external edit, parse and apply to block store

This is a timing-window-free echo suppression pattern. No timestamps, no debouncing.

### Sync Flow (file → blocks)

1. `on_file_changed(path)` — file watcher notifies of change
2. Read disk content, compare to `last_projection`
3. If external: `parse_org_file(content)` → blocks
4. Diff against current DB state via `blocks_differ()`
5. Dispatch create/update/delete operations via `command_bus`
6. Update `last_projection`

### Sync Flow (blocks → file)

1. `on_block_changed(doc_id)` — CDC event from Turso
2. Load all blocks for the document via `block_reader`
3. `OrgRenderer::render(blocks)` → org text
4. Write to disk
5. Set `last_projection[file] = rendered_text`

### blocks_differ

Checks content, parent_id, content_type, source_language, source_name, task_state, priority, tags, scheduled, deadline, org_properties. Returns `true` if any field differs.

### AliasRegistrar

Callback trait for registering `doc_id → file path` aliases in the Loro store. Decouples `OrgSyncController` from Loro.

## Parser

`crates/holon-orgmode/src/parser.rs` — parses org-mode files into `Block` trees.

Key behavior: **bare ID convention**. Org files store IDs without scheme prefixes. The parser adds `block:` scheme via `EntityUri::block(id)` for source blocks. `#+PROPERTIES:ID: abc123` → `EntityUri::block("abc123")`.

`generate_file_id(path)` — deterministic ID from file path for document entity.

`parse_org_file(content)` → `Vec<Block>` with proper `parent_id` chains.

## OrgRenderer

`crates/holon-orgmode/src/org_renderer.rs` — **THE ONLY path** for producing org text from blocks. No other component writes org files.

Renders: headings (with depth-appropriate `*` count), source blocks (`#+BEGIN_SRC lang\n...\n#+END_SRC`), task states (`TODO`, `DONE`, etc.), priorities `[#A]`, scheduled/deadline timestamps, properties drawers, tags.

Source blocks render **before** text children (org format associates source blocks with the preceding heading).

## File Watcher

`crates/holon-orgmode/src/file_watcher.rs` — wraps `notify` crate for file system events. Watches the `root_dir` recursively. Debounces events to avoid partial-write races.

## Traits (DI boundary)

`crates/holon-orgmode/src/traits.rs`:

```rust
pub trait BlockReader: Send + Sync {
    async fn get_blocks_for_document(&self, doc_id: &EntityUri) -> Result<Vec<Block>>;
}

pub trait DocumentManager: Send + Sync {
    async fn get_document(&self, doc_id: &EntityUri) -> Result<Option<Document>>;
    async fn create_document(&self, doc: Document) -> Result<()>;
    async fn update_document(&self, doc: Document) -> Result<()>;
}
```

DI adapters in `crates/holon-orgmode/src/di.rs` implement these traits using `CacheBlockReader` (wraps `QueryableCache<Block>`) and `DocumentManagerAdapter`.

## Org File Conventions

From `docs/ORG_SYNTAX.md`:
- IDs stored **without** `block:` / `doc:` scheme prefixes
- Source blocks get `EntityUri::block()` prefix at parse time
- Renderer strips scheme: writes `block.id.id()` not `block.id.as_str()`
- Custom properties (hyphenated like `collapse-to`, `column-order`) survive full round-trip

## Related Pages

- [[concepts/org-sync]] — design rationale and echo suppression details
- [[entities/holon-crate]] — `OrgSyncController` is wired in DI lifecycle
- [[concepts/loro-crdt]] — `AliasRegistrar` bridges org sync and Loro
