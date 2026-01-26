---
title: Org-Mode Bidirectional Sync
type: concept
tags: [orgmode, sync, echo-suppression, bidirectional]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-orgmode/src/org_sync_controller.rs
  - crates/holon-orgmode/src/parser.rs
  - crates/holon-orgmode/src/org_renderer.rs
  - crates/holon-orgmode/src/traits.rs
  - docs/ORG_SYNTAX.md
---

# Org-Mode Bidirectional Sync

Holon maintains live bidirectional sync with org-mode files:
- Edits to `.org` files on disk appear in Holon
- Edits in Holon are written back to `.org` files

## Echo Suppression

The central challenge: when Holon writes a file, the file watcher fires. Without suppression, this triggers a re-import loop.

### Solution: Projection + Diff Pattern

`last_projection: HashMap<CanonicalPath, String>` tracks the last content Holon wrote (or confirmed) to each file.

On file change:
1. Read disk content
2. Compare against `last_projection[file]`
3. Equal → our own write, suppress
4. Different → external edit, parse + apply to block store

**No timing windows.** Compare content, not timestamps. `CanonicalPath` resolves macOS `/var → /private/var` symlinks so file watcher events and scan events use the same key.

## Single-Task Architecture

`OrgSyncController` runs on a single tokio task. `on_file_changed` and `on_block_changed` are serialized via `tokio::select!`. No concurrent access to `last_projection`, no locking needed.

## File → Blocks Flow

1. File watcher detects change to `foo.org`
2. `on_file_changed(path)` reads disk
3. Compare to `last_projection` — if external:
4. `parse_org_file(content)` → `Vec<Block>`
5. Load current DB blocks for this document
6. `blocks_differ(old, new)` per block — checks: content, parent_id, content_type, source_language, source_name, task_state, priority, tags, scheduled, deadline, org_properties
7. Dispatch create/update/delete operations via `command_bus`
8. Update `last_projection[file] = disk_content`

## Blocks → File Flow

1. CDC event fires for blocks in document `doc_id`
2. `on_block_changed(doc_id)` loads all blocks for document
3. `OrgRenderer::render(blocks)` → org text
4. Write to disk
5. Set `last_projection[file] = rendered_text`

`OrgRenderer` is **THE ONLY** path for producing org text. No other component writes org files.

## Org File Format Conventions

From `docs/ORG_SYNTAX.md`:

**Bare ID convention**: Org files store IDs WITHOUT scheme prefixes.
- `#+PROPERTIES:ID: abc123` — not `block:abc123`
- Parser adds `block:` scheme via `EntityUri::block(id)` at the boundary
- Renderer strips scheme: writes `block.id.id()` (path part) not `block.id.as_str()`

**Source block render order**: Source blocks render BEFORE text children in the org file. Org format associates source blocks with the nearest preceding heading, so they must appear before any text children.

**Custom properties**: Hyphenated custom properties like `collapse-to`, `column-order`, `ideal-width` survive the full round-trip via the `CacheEventSubscriber` fix (ensures `properties: HashMap` is preserved during `INSERT OR REPLACE`).

### Supported Org Features

| Feature | Org Syntax |
|---------|-----------|
| Task states | `* TODO`, `* DONE`, `* IN-PROGRESS` |
| Priority | `* [#A] Headline` |
| Tags | `* Headline :tag1:tag2:` |
| Scheduled | `SCHEDULED: <2026-04-13>` |
| Deadline | `DEADLINE: <2026-04-13>` |
| Properties | `:PROPERTIES:...:END:` |
| Source blocks | `#+BEGIN_SRC lang\n...\n#+END_SRC` |
| Queries | `#+BEGIN_SRC holon_prql\n...\n#+END_SRC` |
| Render | `#+BEGIN_SRC render\n...\n#+END_SRC` |

## Document Identity

Documents have two URIs:
- File-path-based: `holon-doc://file.org`
- UUID-based: `holon-doc://{uuid}`

`LoroDocumentStore.register_alias(uuid, path)` maps UUID → canonical file path. `OrgSyncController` rewrites root block `parent_id`s from file-based to UUID-based URIs after initial parse.

The root document `index.org` is the parent container (`parent_id = "__no_parent__"`). Child docs have `parent_id = "index.org"`.

## Post-Write Hook

Optional shell command (`post_org_write_hook` in `holon.toml`) runs after each org file write. Useful for git commit, cloud sync triggers, etc.

## DI Decoupling

`OrgSyncController` is decoupled from Loro and Turso via trait boundaries:
- `BlockReader` — `CacheBlockReader` wraps `QueryableCache<Block>`
- `DocumentManager` — `DocumentManagerAdapter` wraps Turso CRUD
- `AliasRegistrar` — `LoroAliasRegistrar` bridges to Loro

This lets the controller be tested without real storage.

## Related Pages

- [[entities/holon-orgmode]] — implementation details
- [[entities/holon-crate]] — DI lifecycle wires the controller
- [[concepts/loro-crdt]] — `AliasRegistrar` bridges org sync and Loro
