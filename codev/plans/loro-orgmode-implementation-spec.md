# Loro + Org-mode Bidirectional Sync - Implementation Spec

## Overview

Implement bidirectional sync between Loro (CRDT) and org-mode files:
- **Loro** = authoritative CRDT store, enables P2P sync via Iroh
- **Org files** = human-readable backup, Git-friendly, external editing (Claude Code, Emacs)

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Loro Document (per org file)                │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ LoroTree (block hierarchy)                               │  │
│  │   └─ Node data: LoroMap                                  │  │
│  │       ├─ uuid: "local://..."   ← stable block ID         │  │
│  │       ├─ content: LoroText     ← block text content      │  │
│  │       └─ properties: LoroMap   ← metadata                │  │
│  └──────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────┘
                            │
         ┌──────────────────┼──────────────────┐
         │                  │                  │
         ▼                  ▼                  ▼
   ┌───────────┐     ┌───────────┐     ┌───────────┐
   │ Org Files │     │ P2P Sync  │     │ Turso     │
   │ (disk)    │     │ (Iroh)    │     │ (cache)   │
   └───────────┘     └───────────┘     └───────────┘
```

## Data Model

### Block in Loro

Each block is a LoroTree node with associated LoroMap data:

```rust
// Conceptual structure (actual storage is in Loro containers)
struct BlockData {
    uuid: String,           // "local://550e8400-e29b-41d4-a716-446655440000"
    content: LoroText,      // Block text content (character-level CRDT)
    properties: LoroMap,    // Arbitrary metadata (tags, dates, etc.)
}
```

**Important**: Loro generates internal `TreeID`/`ContainerID` - these are document-scoped and must NOT be used externally. Always use our `uuid` field for references.

### Block in Org-mode

```org
* Heading Text
:PROPERTIES:
:ID: local://550e8400-e29b-41d4-a716-446655440000
:CREATED: 2024-01-15T10:30:00Z
:END:

Body content with [[local://other-uuid][a link]] inline.

** Child Block
:PROPERTIES:
:ID: local://child-uuid
:END:
Child content here.
```

### Links

ALL links use URI format, even within the same file:
```
[[local://target-uuid][link text]]
```

This ensures links survive when blocks move between files.

## Core Rules

1. **One LoroDoc per org file** - sharing a file = sharing a LoroDoc
2. **Block identity via UUID** - stored in `:ID:` property, globally unique
3. **ALL links are URIs** - no Loro-internal references for links
4. **Tree structure uses LoroTree** - parent-child hierarchy is CRDT-native
5. **Loro persistence is app-internal** - org files are the user-facing backup
6. **External edits apply as minimal ops** - diff-based, never rebuild from scratch

## Existing Code

### Loro Backend (exists, not plugged in)
- `crates/holon/src/api/loro_backend.rs` - Complete LoroBackend implementation
- `crates/holon/src/sync/collaborative_doc.rs` - CollaborativeDoc wrapper with Iroh

### Org-mode Rendering (exists, extensive!)
- `crates/holon-orgmode/src/models.rs` - **ToOrg trait and implementations**
  - `OrgFile::to_org()` - renders `#+TITLE:`, `#+TODO:` etc.
  - `OrgHeadline::to_org()` - renders full headline (level, TODO, priority, title, tags, properties drawer, planning, body, source blocks)
  - `SourceBlock::to_org()` - renders `#+NAME:`, `#+BEGIN_SRC lang :args`, content, `#+END_SRC`
  - Helper functions: `format_tags()`, `format_properties_drawer()`, `format_planning()`, `format_header_args()`, `format_header_args_value()`

### Org-mode Parsing (exists)
- `crates/holon-orgmode/src/parser.rs` - `parse_org_file()` function
  - Parses org content → `OrgFile` + `Vec<OrgHeadline>`
  - Handles TODO keywords, priorities, tags, properties, source blocks

### Round-trip Tests (exist)
- `crates/holon-orgmode/tests/org_round_trip.rs` - Property-based tests
  - Verifies: to_org() → parse() → compare

### Org-mode Sync (exists)
- `crates/holon-orgmode/src/orgmode_sync_provider.rs` - File watcher, change detection

### Cache System (exists)
- `crates/holon/src/core/queryable_cache.rs` - Turso-backed cache

### What's Missing (needs implementation)
1. **`:ID:` property handling** - Store/retrieve our `local://uuid` in properties drawer
2. **Link parsing** - Extract `[[target][text]]` from content
3. **Loro ↔ OrgHeadline conversion** - Bridge LoroTree nodes to existing models
4. **Diff algorithm** - Compare old vs new parsed blocks
5. **Minimal text diff** - Character-level changes for LoroText

## Implementation Phases

### Phase 1: Loro Persistence

**Goal**: Save/load LoroDoc to disk

**Files to modify**:
- `crates/holon/src/sync/collaborative_doc.rs`

**Tasks**:
1. Add `save_to_file(&self, path: &Path) -> Result<()>`:
   ```rust
   pub async fn save_to_file(&self, path: &Path) -> Result<()> {
       let snapshot = self.with_read(|doc| doc.export_snapshot())?;
       tokio::fs::write(path, snapshot).await?;
       Ok(())
   }
   ```

2. Add `load_from_file(path: &Path) -> Result<Self>`:
   ```rust
   pub async fn load_from_file(path: &Path) -> Result<Self> {
       let bytes = tokio::fs::read(path).await?;
       let doc = LoroDoc::new();
       doc.import(&bytes)?;
       Ok(Self::from_doc(doc))
   }
   ```

3. Create `LoroDocumentStore` to manage multiple docs:
   ```rust
   pub struct LoroDocumentStore {
       docs: HashMap<PathBuf, CollaborativeDoc>,  // org file path -> LoroDoc
       storage_dir: PathBuf,                       // where .loro files are stored
   }

   impl LoroDocumentStore {
       pub async fn get_or_load(&mut self, org_path: &Path) -> Result<&CollaborativeDoc>;
       pub async fn save_all(&self) -> Result<()>;
   }
   ```

### Phase 2: Loro → Org Rendering

**Goal**: Render LoroDoc to org file format

**Leverage existing**: `OrgHeadline::to_org()` already renders headlines. We just need to:
1. Convert LoroTree nodes → `OrgHeadline` structs
2. Add `:ID:` property support
3. Wire up change subscription

**Files to modify**:
- `crates/holon-orgmode/src/models.rs` - Add `:ID:` to properties rendering
- `crates/holon-orgmode/src/renderer.rs` (new) - Loro→OrgHeadline conversion

**Tasks**:
1. Ensure `:ID:` property is rendered in properties drawer:
   ```rust
   // In format_properties_drawer or OrgHeadline::to_org
   // The ID should be first in the drawer
   fn format_properties_with_id(uuid: &str, other_props: &str) -> String {
       let mut result = String::from(":PROPERTIES:\n");
       result.push_str(&format!(":ID: {}\n", uuid));
       // ... add other properties
       result.push_str(":END:");
       result
   }
   ```

2. Convert LoroTree node → OrgHeadline:
   ```rust
   fn loro_node_to_headline(node: &LoroTreeNode, file_id: &str, file_path: &str) -> OrgHeadline {
       let data = &node.data;
       let uuid = data.get_string("uuid").unwrap_or_default();
       let content = data.get_text("content").to_string();
       let props = data.get_map("properties");

       // Parse first line as title, rest as body
       let (title, body) = split_title_body(&content);

       let mut headline = OrgHeadline::new(
           uuid.clone(),
           file_id.to_string(),
           file_path.to_string(),
           node.parent_id().unwrap_or(file_id).to_string(),
           node.depth() as i64,
           node.depth() as i64,  // level = depth for org
           0,  // sequence
           title,
       );
       headline.body = body;
       headline.properties = Some(format!(r#"{{"ID":"{}"}}"#, uuid));
       // ... extract task_state, priority, tags from properties
       headline
   }
   ```

3. Create `OrgRenderer` with debounced writes:
   ```rust
   pub struct OrgRenderer {
       write_tracker: WriteTracker,
   }

   impl OrgRenderer {
       pub fn render_doc(&self, doc: &CollaborativeDoc, file_path: &Path) -> String {
           let mut org_text = String::new();
           // Walk LoroTree, convert to OrgHeadline, call to_org()
           for node in doc.tree.nodes() {
               let headline = loro_node_to_headline(&node, ...);
               org_text.push_str(&headline.to_org());
               org_text.push('\n');
           }
           org_text
       }

       pub async fn write_to_file(&mut self, path: &Path, content: &str) -> Result<()> {
           self.write_tracker.mark_our_write(path);
           tokio::fs::write(path, content).await?;
           Ok(())
       }
   }
   ```

### Phase 3: Org → Loro Parsing

**Goal**: Parse org file changes and apply to Loro as minimal operations

**Leverage existing**: `parse_org_file()` already parses to `OrgHeadline` structs with properties.

**Files to modify**:
- `crates/holon-orgmode/src/parser.rs` - Extract `:ID:` from properties
- `crates/holon-orgmode/src/diff.rs` (new) - Diff algorithm

**Tasks**:
1. Extract `:ID:` from parsed properties (already parsed as JSON):
   ```rust
   impl OrgHeadline {
       pub fn get_block_id(&self) -> Option<String> {
           self.properties.as_ref()
               .and_then(|json| serde_json::from_str::<HashMap<String, String>>(json).ok())
               .and_then(|props| props.get("ID").cloned())
       }
   }
   ```

2. Create diff algorithm:
   ```rust
   pub enum BlockDiff {
       Created { block: ParsedBlock, parent_id: Option<String>, after_sibling: Option<String> },
       Deleted { id: String },
       ContentChanged { id: String, old: String, new: String },
       Moved { id: String, new_parent: Option<String>, after_sibling: Option<String> },
       PropertiesChanged { id: String, changes: Vec<(String, Option<String>, Option<String>)> },
   }

   pub fn diff_blocks(old: &[ParsedBlock], new: &[ParsedBlock]) -> Vec<BlockDiff> {
       // Compare by ID, detect additions/deletions/changes/moves
   }
   ```

3. Apply diffs to Loro:
   ```rust
   pub fn apply_diff_to_loro(doc: &mut CollaborativeDoc, diff: &BlockDiff) -> Result<()> {
       match diff {
           BlockDiff::Created { block, parent_id, after_sibling } => {
               let node = doc.tree.create_node(parent_id)?;
               node.data.set("uuid", &block.id.unwrap_or_else(generate_uuid));
               node.data.get_text("content").insert(0, &block.content);
               // ... set properties
           }
           BlockDiff::ContentChanged { id, old, new } => {
               let node = find_node_by_uuid(doc, id)?;
               let text = node.data.get_text("content");
               // Compute character-level diff and apply
               apply_text_diff(&text, old, new);
           }
           // ... other cases
       }
   }
   ```

4. Character-level text diff:
   ```rust
   fn apply_text_diff(loro_text: &LoroText, old: &str, new: &str) {
       // Use a diff algorithm (e.g., similar to diff-match-patch)
       // Generate minimal insert/delete operations
       for op in compute_text_ops(old, new) {
           match op {
               TextOp::Insert { pos, text } => loro_text.insert(pos, &text),
               TextOp::Delete { pos, len } => loro_text.delete(pos, len),
           }
       }
   }
   ```

### Phase 4: Change Stream Integration (Leverage Existing!)

**Goal**: Connect existing org-mode change stream to Loro

**Existing infrastructure** (no need to build):
- `OrgModeSyncProvider` - already watches files, emits changes
- `OrgHeadlineOperations.watch_changes_since()` - streams `Change<OrgHeadline>`
- `Change<T>` enum - Created, Updated, Deleted, FieldsChanged

**Files to create**:
- `crates/holon/src/sync/loro_org_bridge.rs` (new)

**Tasks**:
1. Create bridge that subscribes to org changes and applies to Loro:
   ```rust
   pub struct LoroOrgBridge {
       doc_store: Arc<LoroDocumentStore>,
       write_tracker: WriteTracker,
   }

   impl LoroOrgBridge {
       pub async fn start(&self, headline_ops: &OrgHeadlineOperations) {
           let mut stream = headline_ops.watch_changes_since(StreamPosition::Beginning).await;

           while let Some(result) = stream.next().await {
               match result {
                   Ok(changes) => {
                       for change in changes {
                           self.apply_change_to_loro(change).await;
                       }
                   }
                   Err(e) => tracing::error!("Change stream error: {}", e),
               }
           }
       }

       async fn apply_change_to_loro(&self, change: Change<OrgHeadline>) {
           // Check if this was our write (Loro → Org → back)
           if self.write_tracker.is_our_write(&change) {
               return;
           }

           match change {
               Change::Created { data, .. } => {
                   // Get or create LoroDoc for this file
                   let doc = self.doc_store.get_or_create(&data.file_path).await?;
                   // Create node in LoroTree with headline data
                   self.create_loro_node(doc, &data).await?;
               }
               Change::Updated { id, data, .. } => {
                   let doc = self.doc_store.get_or_create(&data.file_path).await?;
                   // Find node by UUID, apply text diff
                   self.update_loro_node(doc, &id, &data).await?;
               }
               Change::Deleted { id, .. } => {
                   // Find which doc contains this ID, delete node
                   self.delete_loro_node(&id).await?;
               }
               Change::FieldsChanged { entity_id, fields, .. } => {
                   // Apply field-level changes
                   self.update_loro_fields(&entity_id, &fields).await?;
               }
           }
       }
   }
   ```

2. Track "our writes" to avoid sync loops:
   ```rust
   pub struct WriteTracker {
       recent_loro_writes: HashMap<String, Instant>,  // headline_id → write time
   }

   impl WriteTracker {
       pub fn mark_loro_write(&mut self, headline_id: &str) {
           self.recent_loro_writes.insert(headline_id.to_string(), Instant::now());
       }

       pub fn is_our_write(&self, change: &Change<OrgHeadline>) -> bool {
           let id = match change {
               Change::Created { data, .. } => &data.id,
               Change::Updated { id, .. } => id,
               Change::Deleted { id, .. } => id,
               Change::FieldsChanged { entity_id, .. } => entity_id,
           };
           self.recent_loro_writes.get(id)
               .map(|t| t.elapsed() < Duration::from_secs(2))
               .unwrap_or(false)
       }
   }
   ```

3. Convert OrgHeadline → Loro node data:
   ```rust
   fn headline_to_loro_data(headline: &OrgHeadline) -> HashMap<String, Value> {
       let mut data = HashMap::new();
       data.insert("uuid".to_string(), Value::String(headline.id.clone()));
       // Combine title + body as content
       let content = format!("{}\n{}", headline.title, headline.body.as_deref().unwrap_or(""));
       data.insert("content".to_string(), Value::String(content));
       // ... other properties
       data
   }
   ```

### Phase 5: Link Parsing

**Goal**: Parse `[[target][text]]` links in content

**Tasks**:
1. Add link extraction to parser:
   ```rust
   pub struct Link {
       pub target: String,      // "local://uuid"
       pub text: String,        // display text
       pub start: usize,        // position in content
       pub end: usize,
   }

   pub fn extract_links(content: &str) -> Vec<Link> {
       // Regex: \[\[([^\]]+)\]\[([^\]]+)\]\]
       // Returns all [[target][text]] matches with positions
   }
   ```

2. Store extracted links in block data:
   ```rust
   // During cache sync to Turso
   let links: Vec<String> = extract_links(&content)
       .iter()
       .map(|l| l.target.clone())
       .collect();

   block_entity.links_json = serde_json::to_string(&links)?;
   ```

### Phase 6: Conflict Resolution

**Goal**: Handle delete + edit conflicts

**Tasks**:
1. Implement resurrect-on-edit:
   ```rust
   fn apply_external_edit(&mut self, block_id: &str, new_content: &str) -> Result<()> {
       match self.find_node_by_uuid(block_id) {
           Some(node) => {
               // Block exists, apply edit
               apply_text_diff(&node.content, &node.content.to_string(), new_content);
           }
           None => {
               // Block was deleted in Loro but edited externally
               // Resurrect it
               self.create_block_with_uuid(block_id, new_content)?;
           }
       }
       Ok(())
   }
   ```

## Key Utilities Needed

### UUID Generation
```rust
pub fn generate_block_uuid() -> String {
    format!("local://{}", uuid::Uuid::new_v4())
}
```

### Find Node by UUID
```rust
fn find_node_by_uuid(doc: &CollaborativeDoc, uuid: &str) -> Option<LoroTreeNode> {
    doc.tree.nodes().find(|n| n.data.get_string("uuid") == Some(uuid))
}
```

### Text Diff (use existing crate)
Consider `similar` or `dissimilar` crate for computing minimal text diffs.

## Testing Strategy

1. **Unit tests**: Each phase independently
2. **Round-trip test**: Loro → org → edit org → Loro → verify content
3. **Concurrent edit test**: Simulate external edit + P2P change, verify merge

## Dependencies

Already in Cargo.toml:
- `loro` - CRDT library
- `orgize` - Org-mode parser (used in holon-orgmode)

May need to add:
- `similar` or `dissimilar` - Text diffing
- `notify` - File system watching (check if already present)
