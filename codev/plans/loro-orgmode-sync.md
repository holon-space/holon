# Implementation Plan: Loro + Org-mode Bidirectional Sync

## Goal

Reintroduce Loro as the authoritative CRDT store for internally-managed blocks, with bidirectional sync to org-mode files. This enables:
- P2P sync between Holon instances (via Iroh)
- Backup to Git (org files on disk)
- External editing (Claude Code, Emacs)
- Character-level merge via LoroText

## Key Architectural Decisions (Foundational - Hard to Change)

### 1. One LoroDoc per Org File
- Natural mapping: share a file = share a LoroDoc
- User organizes files freely
- Cross-file moves are copies (acceptable trade-off)

### 2. Block Identity: Application-Level URIs
```
LoroTree Node:
  TreeID: <loro-generated, opaque>     ← Internal, never referenced externally
  data: LoroMap
    uuid: "local://550e8400-..."       ← OUR stable ID, globally unique
    content: LoroText
    properties: LoroMap
```
- Loro generates TreeID/ContainerID - document-scoped, not stable across docs
- We store our own UUID in the node's data - globally unique, survives moves
- All external references use our UUID, never Loro's internal IDs

### 3. ALL Links Use URIs (Consistent Model)
```org
* Block A
Content with [[local://uuid-of-b][link to B]] here.
```
- Even within same file, links are URI strings in LoroText
- No special casing for within-doc vs cross-doc
- Moving block to new file: no link rewriting needed (URI unchanged)
- Tree structure (parent-child) still uses Loro-native LoroTree

### 4. Backlinks (Deferred)
Backlinks are important but deferred. Ideal solution:
- Block entity has `links: Vec<String>` field (extracted during cache sync)
- Turso MatView with UNNEST creates normalized `block_links` view
- Backlinks = query on MatView

**Status**: Waiting for UNNEST support in Turso DBSP MatViews.
Potential PR to add this capability.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     Loro Document (per org file)                │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ LoroTree (block hierarchy)                               │  │
│  │   └─ Node data: LoroMap                                  │  │
│  │       ├─ uuid: "local://..."   ← stable ID               │  │
│  │       ├─ content: LoroText     ← with URI links inline   │  │
│  │       └─ properties: LoroMap                             │  │
│  └──────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────┘
                            │
         ┌──────────────────┼──────────────────┐
         │                  │                  │
         ▼                  ▼                  ▼
   ┌───────────┐     ┌───────────┐     ┌───────────┐
   │ Org Files │     │ P2P Sync  │     │ Turso     │
   │ (disk)    │     │ (Iroh)    │     │ (cache)   │
   └─────┬─────┘     └───────────┘     └─────┬─────┘
         │                                   │
   File watcher                        block_links table
   detects external                    (forward links)
   edits                                     │
                                       Backlinks = query
```

## Data Flow

### Write Path (Holon → Org)
1. User operation → Loro mutation
2. Loro emits change event
3. OrgRenderer listens, re-renders affected file(s)
4. Write to disk (with debounce buffer ~500ms)
5. Mark as "our write" to avoid sync loop

### Read Path (Org → Loro)
1. File watcher detects change
2. Check if "our write" → ignore
3. Parse org file → extract blocks with IDs
4. Diff against current Loro state
5. Apply diff as Loro operations
6. Loro CRDT handles any P2P conflicts

### Link Handling
1. Parse `[[target][text]]` syntax during org→Loro import
2. Store as LoroText marks (range annotations)
3. On block change → re-extract links → update `block_links` table in Turso
4. Backlinks = `SELECT source FROM block_links WHERE target = ?`

## Implementation Phases

### Phase 1: Loro Persistence & DI Integration

**Files to modify:**
- `crates/holon/src/api/loro_backend.rs` - Add persistence (save/load)
- `crates/holon/src/di/mod.rs` - Wire LoroBackend into DI
- `crates/holon/src/sync/collaborative_doc.rs` - Add file I/O

**Tasks:**
1. Add `save_to_file(&self, path: &Path)` to CollaborativeDoc
2. Add `load_from_file(path: &Path)` to CollaborativeDoc
3. Create `LoroDocumentStore` that manages document lifecycle
4. Wire into BackendEngine DI system
5. Add FFI bridge methods for Loro operations

**Persistence format:**
- Use Loro's native snapshot export (binary, compact)
- Store as `.holon` files alongside `.org` files
- Or single `holon.loro` file for all documents

### Phase 2: Org-mode Rendering (Loro → Org)

**Files to create/modify:**
- `crates/holon-orgmode/src/renderer.rs` (new)
- `crates/holon-orgmode/src/writer.rs` (extend)

**Tasks:**
1. Create `OrgRenderer` that subscribes to Loro changes
2. Implement Block → Org headline conversion
3. Handle `:ID:` property for block identity
4. Implement debounced write buffer (500ms)
5. Track "our writes" to avoid sync loops

**Org format for blocks:**
```org
* Heading Text
:PROPERTIES:
:ID: local://550e8400-e29b-41d4-a716-446655440000
:CREATED: 2024-01-15T10:30:00Z
:END:

Body content with [[local://other-block-id][a link]] inline.

** Child Block
:PROPERTIES:
:ID: local://child-uuid
:END:
```

### Phase 3: Org-mode Parsing (Org → Loro)

**Files to modify:**
- `crates/holon-orgmode/src/parser.rs` - Extend for links, IDs
- `crates/holon-orgmode/src/orgmode_sync_provider.rs` - Connect to Loro

**Tasks:**
1. Parse `:ID:` properties to identify blocks
2. Parse `[[target][text]]` links in body text
3. Create diff algorithm: old blocks vs new blocks
4. Convert diff to Loro operations:
   - New block (no ID in Loro) → `loro.create_block()`
   - Modified block (ID exists) → `loro.update_block()`
   - Deleted block (ID in Loro, not in file) → `loro.delete_block()` (with resurrect check)
   - Moved block (different parent) → `loro.move_block()`
5. Handle link marks in LoroText

### Phase 4: File Watcher Integration

**Files to modify:**
- `crates/holon-orgmode/src/orgmode_sync_provider.rs`

**Tasks:**
1. Integrate with existing file watcher (WalkDir-based)
2. Add "our write" detection:
   ```rust
   struct WriteTracker {
       pending_writes: HashSet<PathBuf>,
       write_times: HashMap<PathBuf, Instant>,
   }
   ```
3. On file change:
   - If in `pending_writes` and recent → ignore (our write)
   - Else → trigger Org→Loro sync
4. Debounce rapid changes (100ms)

### Phase 5: Link Parsing (Without Backlink Index)

**Tasks:**
1. Parse `[[target][text]]` links in org content during import
2. Store links in LoroText (as marked ranges or inline syntax)
3. Extract `links: Vec<String>` field during Turso cache sync
4. Backlink index deferred until Turso UNNEST MatView support

### Phase 6: Conflict Resolution

**Tasks:**
1. Implement resurrect-on-edit policy:
   ```rust
   fn apply_external_edit(&self, block_id: &str, new_content: &str) {
       if self.loro.is_deleted(block_id) {
           self.loro.resurrect_block(block_id)?;
       }
       self.loro.update_content(block_id, new_content)?;
   }
   ```
2. Handle concurrent P2P + external file edits
3. After merge → re-render org files to reflect merged state

## Key Design Decisions

### Block Identity
- Use UUID-based URIs: `local://550e8400-...`
- Store in org `:ID:` property
- New blocks without ID get one assigned on import

### LoroText for Content
- Block content stored as LoroText (character-level CRDT)
- Links stored as marks/annotations on text ranges
- Plain string extracted for org rendering

### Sync Direction Priority
- Loro is authoritative for CRDT semantics
- External edits are "just another peer"
- Org files re-rendered after any merge

### File Organization
- One `.org` file per "document" or logical grouping
- `.holon` snapshot file for Loro state (or embedded in app data)
- User controls org file structure (create/rename/delete files)

## Testing Strategy

1. **Unit tests**: Loro operations, org parsing, rendering
2. **Integration tests**: Full round-trip (Loro → org → edit → Loro)
3. **Property-based tests**: Random edits, verify convergence
4. **Conflict scenarios**:
   - Edit same block in org and via P2P simultaneously
   - Delete block in one place, edit in another
   - Move block to different parent in both places

## Resolved Questions

1. **File granularity**: User organizes freely. One LoroDoc per org file.
2. **Block identity**: Application-level URIs (`local://uuid`), stored in LoroMap, not Loro's internal IDs.
3. **Link model**: ALL links use URIs, even within same file. Consistent, no rewriting on move.
4. **Backlinks**: Deferred. Plan: `links` field on block → Turso MatView with UNNEST (pending Turso PR).
5. **Cross-doc refs**: Work via URIs. Moving block to new doc = copy (loses CRDT history, keeps URI).
6. **Conflict policy**: Resurrect deleted blocks if edited externally.
7. **Loro persistence**: App-internal (org files are the user-facing backup).

## Open Questions

1. **ID collision**: What if user manually edits `:ID:` to duplicate value? (Probably: detect and warn/error)
2. **Non-headline blocks**: How to represent source blocks, tables in org? (Future: extend BlockContent enum)
3. **Performance**: Large documents - incremental rendering vs full re-render?
4. **Loro persistence location**: Single file per workspace? Alongside each org file?

## Workflow Considerations

### External Edits (Git, Claude Code, Emacs, etc.)
All external edits use the same sync mechanism:
1. File watcher detects org file change
2. Parse new org content → block structures
3. Diff against current Loro state
4. Generate **minimal Loro operations** (character-level for text, node-level for tree)
5. Apply operations to Loro → CRDT history grows

This means:
- Git pull with merged file → diff → minimal ops (history preserved, new ops added)
- Claude Code edit → same mechanism
- Never "rebuild from scratch" - always incremental

Note: Git merges happen on text, not CRDT ops. The merged result is imported as new ops.
This is fine - Loro history continues, Git provides its own history layer.

### Concurrent External + P2P Edits
When external edit (AI, Git, etc.) happens while P2P peer also edits:
- External changes → parsed → applied as Loro ops
- P2P changes → arrive as Loro ops
- Loro CRDT merges both automatically
- Org file re-rendered after merge reflects combined state

### Fork Scenarios (Rare)
- Forking a doc creates duplicate UUIDs across docs
- Future extension if needed: `local://uuid?doc_id=abc` parameter
- For now: accept that forking = new identity, or manually assign new UUIDs

## Dependencies

- `loro` crate (already in Cargo.toml)
- `orgize` crate (already used in holon-orgmode)
- `notify` crate for file watching (check if already present)
- Existing: `holon-orgmode`, `QueryableCache`, `TursoBackend`

## Estimated Scope

- Phase 1 (Persistence): Foundation work
- Phase 2 (Loro→Org): Core rendering
- Phase 3 (Org→Loro): Core parsing + diff
- Phase 4 (File watcher): Integration
- Phase 5 (Links): Feature addition
- Phase 6 (Conflicts): Edge cases

Phases 1-4 are the minimum viable implementation. Phases 5-6 can follow.
