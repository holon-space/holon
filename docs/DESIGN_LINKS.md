# Links & Backlinks Design

## Problem

Holon needs `[[...]]` wiki-style links that:
1. Survive renames and moves (non-fragile)
2. Allow referencing entities that don't exist yet (creation intent)
3. Work across entity types (documents, blocks, persons, ...)
4. Support efficient backlink queries
5. Converge — multiple blocks mentioning the same not-yet-created entity must resolve to the same entity

Portability of raw org files is a non-goal. Rename-proofness is the top priority.

## Core Idea: Deterministic Hashed IDs

The identity of a linked entity is a **deterministic hash** of its canonical creation path. The hash is formatted as a UUID-style string (matching existing block IDs like `7fc2b308-403a-458e-91a8-0a26fc519320`) but is reproducible from the same input.

```
[[Projects/New thing]]
→ normalize: "doc:projects/new thing"
→ hash: blake3("doc:projects/new thing")
→ format as UUID: 7fc2b308-403a-458e-91a8-0a26fc519320
→ ID: doc:7fc2b308-403a-458e-91a8-0a26fc519320
```

This gives us:
- **Convergence**: same input always produces the same ID. No dedup logic needed.
- **No re-resolution**: links don't need to be re-resolved when entities are created.
- **Rename-proof**: the ID is a hash of the *original* creation path, never changes.
- **No readability misconceptions**: looks like existing block UUIDs, nobody will treat it as semantic.

## Link Lifecycle

### Phase 1: Creation Intent (during editing)

User types a link using natural syntax:

```org
Check out [[Projects/New thing]] for details.
See [[Person/Alice]] about this.
Related to [[Some page]].
```

No entity is created yet. The link text encodes a **creation intent**: type, hierarchy, and name.

### Phase 2: Save (persist the intent)

On save (org file write, Loro sync), the system parses `[[...]]` links, classifies them, and stores them in the `block_link` table:

| `target_raw` | `target_id` | Status |
|---|---|---|
| `Projects/New thing` | `doc:7fc2b308-...` | Unresolved (entity doesn't exist) |
| `doc:7fc2b308-...` | `doc:7fc2b308-...` | Resolved |
| `https://example.com` | `NULL` | External (not an entity) |

The deterministic hash is computed at parse time. `target_id` is always known, even before the entity exists. Backlinks work immediately.

The link text in the content is **not rewritten** at this stage. It stays as the user typed it.

### Phase 3: Entity Creation (lazy, on navigate)

The entity is created when the user **navigates to the link** (clicks it). Not on save — to avoid zombie entities.

On navigate:
1. Compute `target_id` from link text (deterministic — same hash)
2. Check if entity exists
3. If not: create it (document, person, etc.) with the hashed ID
4. Rewrite the source link to resolved form: `[[doc:7fc2b308-...]]`
5. Navigate to the entity

Note: the resolved form stores **only the ID**, no display text. The display name is resolved at render time from the entity's current `name` field. This means renames are free — no propagation needed.

### Phase 4: Resolved Reference (steady state)

```org
Check out [[doc:7fc2b308-403a-458e-91a8-0a26fc519320]] for details.
```

The entity exists. The link is stable. The renderer resolves the ID to the current document name for display. Renaming the document requires zero link updates — every reference automatically shows the new name.

Exception: `[[target][explicit text]]` with user-provided display text is preserved as-is. This is for cases where the user intentionally wants different link text (e.g., `[[doc:7fc2b308-...][click here]]`). Only the renderer-resolved default name updates on rename.

## Type Inference

The first path segment is checked against a type registry. If it matches a known entity type, it determines the scheme. Otherwise, the default is `doc:`.

| User types | Inferred type | Hash input | Resulting ID |
|---|---|---|---|
| `[[Projects/New thing]]` | `doc` (default) | `"doc:projects/new thing"` | `doc:7fc2b308-...` |
| `[[Person/Alice]]` | `person` (registered) | `"person:alice"` | `person:e91c4a2f-...` |
| `[[Some page]]` | `doc` (default, root) | `"doc:some page"` | `doc:b4e8f1a3-...` |
| `[[doc:existing-id]]` | — (already resolved) | — | `doc:existing-id` |
| `[[https://example.com]]` | — (external URL) | — | `NULL` |

The type registry is a simple map: `{"Person" => "person:", "Project" => "project:", ...}`. Initially just `doc:` and `block:`. Grows as entity types are added (person, monetary, etc. per Vision/PetriNet.md).

For inferred types, the type prefix is stripped from the path before it becomes the hierarchy hint. `[[Person/Alice]]` creates an entity with scheme `person:`, name "Alice", no parent path. `[[Projects/Sub/Thing]]` creates `doc:` with name "Thing", parent path "Projects/Sub".

## Hash Function

**Input normalization** (before hashing):
1. Lowercase the entire string
2. Trim whitespace
3. Collapse multiple spaces to single space
4. Prepend scheme if not present: `"doc:projects/new thing"`

**Hash**: Blake3 (or any fast hash already in the dependency tree), formatted as UUID-style hex with dashes to match existing block ID format.

**Implementation**: Pure function in `holon-api` (no dependencies on storage):

```rust
// holon_api::link_parser
pub fn deterministic_entity_id(scheme: &str, normalized_path: &str) -> EntityUri {
    let input = format!("{}:{}", scheme, normalized_path.to_lowercase().trim());
    let hash = blake3::hash(input.as_bytes());
    let bytes = hash.as_bytes();
    let uuid_str = format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]),
        u16::from_be_bytes([bytes[8], bytes[9]]),
        // 6 bytes for the last segment
        u64::from_be_bytes([0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]])
    );
    EntityUri::new(scheme, &uuid_str)
}
```

## Convergence

Multiple blocks referencing `[[Projects/New thing]]` all compute the same `target_id`:

```
Block A: "Check out [[Projects/New thing]]"     → target_id = doc:7fc2b308-...
Block B: "Related to [[Projects/New thing]]"     → target_id = doc:7fc2b308-...
Block C: "See also [[projects/new thing]]"       → target_id = doc:7fc2b308-... (case-insensitive)
```

When any of these links is navigated, the entity is created with ID `doc:7fc2b308-...`. All three links resolve to the same entity. No dedup logic needed.

`[[New thing]]` (without path qualifier) produces a *different* hash (`"doc:new thing"` vs `"doc:projects/new thing"`). This is correct — unqualified names are a different creation intent (root-level document). The autocomplete UI should help users pick the right qualified path.

**Autocomplete and case correction**: When a document named "Projects" already exists, autocomplete should match case-insensitively but insert the canonical casing. Typing `[[projects/` shows "Projects" and inserts `[[Projects/`. Case normalization in the hash is a safety net for raw org editing; the UI enforces correct casing through autocomplete.

## Storage

### In Turso (`block_link` table)

```sql
CREATE TABLE IF NOT EXISTS block_link (
    source_block_id TEXT NOT NULL,
    target_raw TEXT NOT NULL,          -- original text from [[...]]
    target_id TEXT,                    -- deterministic hash ID (always set for entity links, NULL for external URLs)
    display_text TEXT,                 -- user-provided override from [[target][text]], NULL if none
    position INTEGER NOT NULL,         -- byte offset in source content
    PRIMARY KEY (source_block_id, position)
);

CREATE INDEX IF NOT EXISTS idx_block_link_source ON block_link(source_block_id);
CREATE INDEX IF NOT EXISTS idx_block_link_target_id ON block_link(target_id);
```

No backlinks matview needed. The two indexes support both directions efficiently:
- **Forward links**: `SELECT * FROM block_link WHERE source_block_id = ?`
- **Backlinks**: `SELECT * FROM block_link WHERE target_id = ?`

A JOIN to `block` for context (source content, source document) is cheap with the existing `block` primary key index and not worth the IVM overhead of a materialized view at PKM scale.

### In org files

Two forms coexist:

```org
* Unresolved (creation intent — entity doesn't exist yet)
Check out [[Projects/New thing]] for details.

* Resolved (after navigate-to-create — entity exists)
Check out [[doc:7fc2b308-403a-458e-91a8-0a26fc519320]] for details.

* External (never resolved)
See [[https://example.com][example site]] for reference.
```

### In Loro

Same as org file content. The `[[...]]` text is part of the block's `content` field. No separate links field needed — links are extracted by the parser at the event boundary.

### Display name resolution

Resolved links like `[[doc:7fc2b308-...]]` store only the ID. The renderer resolves the display name at render time:

1. Look up entity by ID in cache
2. Use entity's `name` field as display text
3. If entity doesn't exist (dangling reference), show the raw ID with visual indicator

This is the same approach LogSeq uses — display names always reflect the current entity name, never go stale.

For links with explicit user-provided display text (`[[doc:7fc2b308-...][click here]]`), the explicit text takes precedence.

## Entity Creation Flow

When the user navigates to an unresolved link:

```
User clicks [[Projects/New thing]]
    ↓
Compute target_id = deterministic_entity_id("doc", "projects/new thing")
    ↓
Check: does entity with this ID exist?
    ├── Yes → navigate to it
    └── No → create it:
            1. Parse path: parent = "Projects", name = "New thing"
            2. Resolve parent document (create recursively if needed)
            3. Create document: id = target_id, name = "New thing", parent_id = resolved parent
            4. Rewrite source link to [[doc:7fc2b308-...]]
            5. Navigate to new entity
```

Creation is an event through the OperationProvider, not direct SQL. This triggers the EventBus, which updates caches, matviews, etc.

## UI Behavior

### Autocomplete

When typing `[[`, the UI offers:
- Existing entities (documents, blocks, persons) — filtered by typed text
- "Create new: Projects/New thing" option if no match

Selecting an existing entity inserts the resolved form directly: `[[doc:7fc2b308-...]]`.

Selecting "create new" inserts the unresolved form: `[[Projects/New thing]]`.

Autocomplete enforces canonical casing: typing `projects/` matches the existing "Projects" document and inserts `[[Projects/`.

### Visual distinction

| Link state | Appearance |
|---|---|
| Resolved, entity exists | Normal link (blue, clickable), display name from entity |
| Unresolved, entity doesn't exist | Dashed underline, muted color, display name from link text |
| External URL | External link icon, display from `[text]` or URL |

### Rename handling

Renames require **zero link updates**. Since resolved links store only the ID and the display name is resolved at render time, renaming a document immediately updates how all links to it are displayed. No batch updates, no stale text.

## Architectural Boundaries

| Layer | Responsibility |
|---|---|
| **Link parser** (`holon-api/src/link_parser.rs`) | Extract `[[...]]` links, classify as resolved/unresolved/external, compute deterministic IDs |
| **ID generator** (`holon-api`) | `deterministic_entity_id(scheme, path)` — pure function, no IO |
| **Type registry** (`holon-api`) | Map path prefixes to entity schemes |
| **LinkEventSubscriber** (`holon/src/sync/`) | Populate `block_link` on block events (no resolution queries — just hash computation) |
| **Entity creation** (OperationProvider) | Create entities via events, not direct SQL |
| **Display name resolution** (frontend/renderer) | Resolve `doc:id` → entity name at render time |
| **Navigation** (frontend) | Navigate-to-create flow |
| **Autocomplete** (frontend) | Entity search + "create new" option + case correction |
| **OrgSyncController** | Untouched — continues to emit block events |

## What Changes from Current Implementation

The current implementation (in `link_event_subscriber.rs`) uses `target_document_id` with name-based resolution and re-resolution on document creation. This design replaces that with:

1. `target_document_id` → `target_id` (deterministic hash, always known at parse time)
2. Name-based resolution (`SELECT id FROM document WHERE name = ?`) → deterministic hash computation (no DB queries)
3. Re-resolution on document creation → not needed (removed)
4. `resolve_pending_links` → not needed (removed)
5. Document event subscription in LinkEventSubscriber → not needed (removed)
6. `backlinks` materialized view → not needed (removed, use `block_link` with index)
7. `display_text` semantics change: now only stores user-provided overrides, not the entity name
8. `resolve_target` / `resolve_by_path` / `resolve_by_name` → replaced by `deterministic_entity_id` (pure function, no IO)

## Decisions Made

1. **Display text**: Store only the ID in resolved links. Resolve display name at render time from entity's current name. Renames are free. (LogSeq approach.)

2. **Backlinks table**: No matview. `block_link` with indexes on `source_block_id` and `target_id` supports both directions. JOIN to `block` at query time is cheap enough.

3. **Case handling**: Hash normalizes to lowercase (safety net). Autocomplete enforces canonical casing from existing entities (primary UX).

4. **Block-level links**: `[[block:some-id]]` references a block directly by its existing EntityUri. No hashing needed.

5. **Hash format**: UUID-style hex with dashes, matching existing block ID format.

6. **Type registry**: Hardcoded initially (`doc:`, `block:`). Made configurable later as entity types grow.

7. **Hash algorithm**: Blake3 or whatever fast hash is already in the dependency tree. Choice doesn't matter at PKM scale.
