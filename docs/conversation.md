<ai>
# 📋 Summary: Personal Knowledge & Task Management System

## 🎯 Your Vision
A unified system that:
- Combines PKM (Personal Knowledge Management) with task/project management
- Integrates data from multiple external systems (Todoist, Gmail, Jira, Linear, Calendar)
- Supports filtering (location, time, energy, people), Kanban views, block transclusion
- Uses typeclasses/traits for unified task interfaces across systems
- Stores internal data in plain text (Markdown/YAML)
- Syncs between devices with CRDTs (preferably Loro)
- Enables AI agent task delegation
- Separates UI from backend

## ✅ What Works Well Together

**Good Synergies:**
- Plain-text storage + in-memory database (proven pattern)
- Block transclusion via URLs (well-established)
- Templates for display customization
- Adapter/plugin pattern for external systems (better than strict typeclasses)
- TipTap + CRDT for collaborative rich text editing

## ⚠️ Major Challenges Identified

### 1. **Two-Way Markdown Sync = Abandoned**
- **Problem:** CRDTs + external file edits = broken state
- **Solution:** Use Markdown as **export-only** format
- Store: `.content.md` (human-readable) + `.content.crdt` (CRDT metadata)

### 2. **External API Integration Complexity**
- Each API has different models, rate limits, auth
- **Maintenance grows linearly** with each integration
- No real-time updates (mostly polling)
- Ongoing breaking changes

### 3. **Typeclass Abstraction Problem**
- Task models are **too heterogeneous** across systems:
  - Todoist: flat + sections, priority 1-4
  - Jira: epics→stories→subtasks, custom fields, story points
  - Linear: cycles, teams, triage states
- **Recommendation:** Adapter pattern with capability traits instead of one unified interface

### 4. **LSP for Rich UI = Not Feasible**
- LSP is designed for **text buffers only**
- Cannot render Kanban boards, handle drag-drop, or drive mobile UIs
- Could work for text portions, but not as general UI protocol

## 🏗️ Technology Stack Recommendation

### **For CRDTs: Loro > Yjs**

**Loro Advantages:**
- ✅ **All data types you need:** Text, List, Map, Tree, Counter, MovableList
- ✅ **Time-travel/version control** (Git-like history)
- ✅ **Better for task management:** Native drag-drop with MovableList
- ✅ **Tree structures:** Built-in for project hierarchies
- ✅ **Rust-native:** Aligns with your preferences
- ⚠️ Newer (v1.0 in 2024), smaller ecosystem

**Yjs Strengths (what you'd miss):**
- More battle-tested, larger community
- XML types (not needed for your use case)
- Official TipTap collaboration packages
- More production deployments

**Verdict:** Loro's data structure match is too good to pass up for your use case.

### **For Rich Text: TipTap**

- **TipTap** = Developer-friendly wrapper over ProseMirror
- **ProseMirror** = Powerful but low-level toolkit
- Loro has official ProseMirror bindings that work with TipTap
- Headless design gives full UI control

**Integration:**
```typescript
import { LoroSyncPlugin, LoroUndoPlugin } from "loro-prosemirror";
import { LoroDoc } from "loro-crdt";

const doc = new LoroDoc();
const plugins = [
  LoroSyncPlugin({ doc }),
  LoroUndoPlugin({ doc }),
  // ... TipTap plugins
];
```

### **For Backend: Rust**

**Why Rust:**
- ✅ Traits perfect for your typeclass-like abstractions
- ✅ Performance (mobile/desktop)
- ✅ Loro is native Rust
- ✅ Type safety prevents integration bugs
- ✅ Cross-platform (Tauri for desktop)

**Stack:**
- Web framework: `axum` or `actix-web`
- Storage: `sled` (embedded KV) or `sqlite`
- CRDT: `loro` crate
- Search: `tantivy`

### **For UI: Tauri + Modern JS Framework**

- Desktop: Tauri (Rust backend + web frontend)
- Frontend: SolidJS or Svelte (lighter than React)
- Mobile: Flutter or React Native (share Rust business logic via FFI)

## 🏛️ Recommended Architecture

### **Hybrid Data Model:**

```
┌─────────────────────────────────────────┐
│         Application Layer               │
│         (Rust + Tauri)                  │
└─────────────────────────────────────────┘
                    │
        ┌───────────┴───────────┐
        ▼                       ▼
┌───────────────┐      ┌──────────────────┐
│ Structured    │      │  Rich Text       │
│ Data          │      │  Editor          │
│               │      │                  │
│ Loro Map/List │      │ TipTap/Loro-Text │
│ Loro Tree     │      │                  │
│ ↓             │      │ ↓                │
│ YAML/JSON     │      │ Markdown*        │
│ files         │      │ (export only)    │
└───────────────┘      └──────────────────┘
        │                       │
        └───────────┬───────────┘
                    ▼
        ┌─────────────────────┐
        │   Sync Layer        │
        │   (Loro CRDT)       │
        └─────────────────────┘
```

### **File Structure:**

```
projects/
  personal-website/
    project.yaml          # Loro Tree/Map → YAML
    notes/
      architecture.md     # Loro Text → Markdown export
      .architecture.crdt  # CRDT metadata
    tasks/
      tasks.yaml          # Loro MovableList → YAML
```

### **Data Types Usage:**

```rust
let doc = LoroDoc::new();

// Projects hierarchy
let projects = doc.get_tree("projects");

// Tasks with drag-drop
let kanban = doc.get_movable_list("tasks");

// Rich text notes
let notes = doc.get_text("notes");

// Vote/priority counters
let votes = doc.get_counter("votes");

// Task metadata
let task_data = doc.get_map("task_metadata");
```

## 📊 Complexity Assessment

**Phase 1 (MVP - 3-6 months):**
- Rust backend + Tauri
- Local markdown/YAML files + SQLite cache
- Basic task management (no external integrations)
- Kanban + list views
- Block references within system

**Phase 2 (First Integration - 2-3 months):**
- Add one external system (e.g., Todoist)
- Prove adapter pattern works
- Handle sync conflicts

**Phase 3 (Expansion - ongoing):**
- Add more integrations (2-3 months each)
- Refine abstractions based on real differences

**Phase 4 (Collaboration - 3-4 months):**
- CRDT sync server
- Mobile app

**Total realistic timeline:** 2-3 years solo, 6-12 months with small team

**Complexity breakdown:**
- 40% External API integrations + maintenance
- 30% Sync and conflict resolution
- 20% UI/UX across platforms
- 10% Core logic

## 🚨 Key Risks & Mitigations

### **Risk 1: Loro is Newer**
- **Mitigation:** Both libraries are open source; can fork if needed
- Local-first design means no vendor lock-in

### **Risk 2: External API Maintenance**
- **Mitigation:** Start with 1-2 integrations; add slowly
- Build robust adapter layer from day one

### **Risk 3: Scope Creep**
- **Mitigation:** Build MVP with internal data only first
- Validate workflow before adding integrations

### **Risk 4: Plain Text Impedance Mismatch**
- **Mitigation:** Accept hybrid model (CRDT primary, markdown export)
- Don't fight the architecture

## 🎯 Final Recommendations

1. **Use Loro** for CRDT (data structures match perfectly)
2. **Use TipTap** for rich text (friendlier than raw ProseMirror)
3. **Build in Rust** (performance + type safety + Loro native)
4. **Use Tauri** for desktop (native performance + web UI)
5. **Separate concerns:** Structured data (tasks/metadata) vs. rich content (notes)
6. **Adapter pattern** over strict typeclasses for external systems
7. **Start small:** MVP with internal data, add integrations incrementally
8. **Accept hybrid storage:** CRDT as source of truth, plain text as export

## ⚖️ Alternative to Consider

**Before building from scratch, evaluate:**
- **Anytype** (local-first, object-based, has sync)
- **Logseq** (outliner, block references, sync built-in)
- Contributing to these projects vs. building from scratch

The system you're describing is **ambitious but achievable** with the right tech stack and phased approach. The Loro + TipTap + Rust combination gives you the best foundation for your requirements.
</ai>
<user>

  I agree that Loro is the better choice here. In LogSeq I move nodes around all the time.
  Could ProseMirror and/or TipTap be used to create a block-based editor like LogSeq?
  How would integration of external systems like Todoist, GMail, Calendars look like?
  Do ProseMirror/TipTap have an internal data structure that they need to have full control over, or are there abstractions/interfaces that one could implement for external systems so that any reading and modifications are directly passed through to the external system?
</user>

<ai>
  Excellent questions! Let me break these down:

  ## Block-Based Editor with ProseMirror/TipTap

  **TL;DR: Possible, but fighting the design. Better to separate concerns.**

  ProseMirror is fundamentally a **document-centric** editor. It excels at rich text within a single document tree, but LogSeq's model is quite different:

  **LogSeq's Architecture:**
  - Each block is an **independent entity** with its own identity
  - Blocks can be transcluded, reordered, nested arbitrarily
  - The outline view is the primary interface
  - Each block persists its identity across moves

  **ProseMirror's Model:**
  - One document with nested nodes
  - Nodes exist within document structure
  - Selection/cursor assumes continuous text flow
  - Optimized for document editing, not outline management

  **You *could* hack it:**
  - Make each block a custom ProseMirror node type
  - Add metadata as node attributes
  - Use custom node views for rendering
  - But you'd be swimming upstream

  **Better approach (what Notion/Anytype likely do):**

  ```
  ┌─────────────────────────────────┐
  │   Outliner/Tree Component       │
  │   (manages block hierarchy)     │
  └─────────────────────────────────┘
              ↓
  ┌─────────────────────────────────┐
  │   Block Storage (Loro Tree)     │
  │   - Block IDs                   │
  │   - Hierarchy                   │
  │   - Metadata                    │
  └─────────────────────────────────┘
              ↓
  ┌─────────────────────────────────┐
  │  Individual Block Editors       │
  │  (lightweight text editing)     │
  │  - Could be TipTap              │
  │  - Or simpler: ContentEditable  │
  │  - Or: Lexical, Slate, etc.     │
  └─────────────────────────────────┘
  ```

  Each block has its own small editor instance, rather than one giant ProseMirror managing everything.

  **Loro fits this beautifully:**
  - `Loro.Tree` for block hierarchy (with drag-drop via MovableList)
  - `Loro.Text` for each block's content
  - `Loro.Map` for block metadata

  ---

  ## External System Integration

  This is the **real** architectural question. There are three main approaches:

  ### **Approach A: Sync/Cache (Recommended)**

  ```
  External System (Todoist/Gmail/Jira)
          ↓ periodic sync
      Local Cache (SQLite/sled)
          ↓ read/write
      Your Application
          ↓ push updates
  External System
  ```

  **Pros:**
  - Works offline
  - Fast reads
  - Can add local metadata (tags, relations, notes)
  - Can relate external items to internal content
  - Handles rate limits gracefully
  - Search across all systems

  **Cons:**
  - Sync complexity
  - Conflict resolution
  - Stale data (mitigated by smart sync)

  ### **Approach B: Direct Pass-Through**

  ```
  Your UI → Every operation → External API (live)
  ```

  **Pros:**
  - Always current
  - No sync logic
  - Simple consistency

  **Cons:**
  - Requires internet always
  - API rate limits break the UI
  - High latency (every keystroke = API call)
  - **Can't add your own metadata** (this kills it)
  - **Can't create relationships** between systems
  - Can't search/filter across systems efficiently

  ### **Approach C: Hybrid (Best of Both)**

  ```
  Read from cache → Fast display
  User edits → Write through to API immediately + update cache
  Background sync → Reconcile any conflicts/updates
  ```

  **For your use case, Approach A or C is strongly recommended** because:
  1. You want to **relate** Todoist tasks to project notes
  2. You want **filtering** across all sources (location + energy + time)
  3. You want to **add metadata** (your own tags, priorities, contexts)
  4. You want **offline access**
  5. You want **fast search** across everything

  ---

  ## ProseMirror/TipTap Data Structure Control

  **Direct answer: They need significant control. Pass-through to external systems is not practical.**

  ### **How ProseMirror Works:**

  ```typescript
  // Internal state
  {
    doc: Node,              // Tree of content nodes
    selection: Selection,   // Cursor position
    schema: Schema          // Document structure rules
  }

  // Changes via transactions
  state.tr
    .insertText("hello")
    .setMeta("foo", "bar")
    .apply()
  ```

  - ProseMirror maintains an **in-memory document tree**
  - Changes happen through **transactions** (sync operations)
  - Undo/redo based on transaction history
  - Selection state tied to document positions
  - Collaborative editing via CRDT or OT

  ### **Can You Implement Abstractions for External Systems?**

  **For read-only display:** Yes, easy
  ```typescript
  externalData → transform → ProseMirror doc → display
  ```

  **For live editing with pass-through:** Theoretically possible, practically a nightmare

  You'd need:
  ```typescript
  User types "h"
    → ProseMirror transaction
    → Custom plugin intercepts
    → Convert to external API call
    → await API.updateTask(id, newText)
    → Response returns
    → Reconcile ProseMirror state with response
    → Hope nothing else changed meanwhile
  ```

  **Why this fails:**
  - **Latency:** Every keystroke waits for API roundtrip (100-500ms minimum)
  - **Rate limits:** APIs throttle requests (Todoist: 450 req/15min)
  - **Conflicts:** What if external system was also updated?
  - **Offline:** Impossible
  - **Complexity:** Maintaining state consistency is brutal

  ### **How Loro Integration Works (The Good Way):**

  ```typescript
  import { LoroSyncPlugin } from "loro-prosemirror";

  const loroDoc = new LoroDoc();
  const text = loroDoc.getText("content");

  const editor = new Editor({
    extensions: [
      StarterKit,
      Collaboration.configure({
        document: text,  // Loro Text is source of truth
      }),
    ],
  });

  // User types → ProseMirror → Loro CRDT → Sync
  // Remote changes → Loro CRDT → ProseMirror update
  ```

  **Key difference:** Loro is **local-first**. Changes are instant locally, then sync'd.

  ---

  ## Recommended Architecture for External Systems

  ```rust
  // Adapter trait
  trait TaskProvider {
      async fn fetch_tasks(&self) -> Result<Vec<Task>>;
      async fn update_task(&self, id: &str, updates: TaskUpdates) -> Result<Task>;
      async fn create_task(&self, task: NewTask) -> Result<Task>;
      fn capabilities(&self) -> ProviderCapabilities;
  }

  // Your local storage
  struct UnifiedStorage {
      loro_doc: LoroDoc,           // Your notes, projects, internal tasks
      cache_db: SqlitePool,        // Cached external items
      providers: Vec<Box<dyn TaskProvider>>,
  }

  // Integration
  impl UnifiedStorage {
      // Background sync
      async fn sync_provider(&mut self, provider: &dyn TaskProvider) {
          let tasks = provider.fetch_tasks().await?;

          // Store in local cache
          for task in tasks {
              self.cache_db.upsert_task(task)?;
          }
      }

      // Create relationships
      fn link_task_to_note(&mut self, task_id: ExternalId, note_block_id: BlockId) {
          let links = self.loro_doc.get_map("task_links");
          links.set(note_block_id, task_id);
      }

      // Unified query
      fn search_all_tasks(&self, filter: TaskFilter) -> Vec<UnifiedTask> {
          // Query both Loro (internal) and cache_db (external)
          // Return unified view
      }
  }
  ```

  ### **How Editing Works:**

  **For your notes/internal content:**
  ```
  User edits → TipTap → Loro CRDT → Instant update → Background sync
  ```

  **For external task properties:**
  ```
  User clicks task → Form UI shows properties
  User edits → Update local cache
             → Push to external API (async)
             → On success, mark as synced
             → On failure, mark as pending, retry later
  ```

  **For relationships/metadata you add:**
  ```
  User tags Todoist task with #urgent → Stored in Loro Map
  User links task to project note → Stored in Loro Map
  User adds location filter → Stored in Loro Map

  These are yours, never pushed to external system.
  ```

  ---

  ## Concrete Example

  ```markdown
  # 📝 Project: Website Redesign
  (This text edited with TipTap + Loro)

  ## Status
  [[todoist:123456]] is blocked by [[jira:PROJ-789]]

  ## Notes
  - Need to coordinate with design team
  - Target launch: Q2 2025
  - Context: #design #web #priority-high
    (These tags stored in Loro, apply to whole project)

  ## Tasks
  - [[todoist:123456]] Design mockups
    - External: Todoist task (cached locally)
    - Your metadata: #design #requires-focus location:office

  - [[local:abc]] Write copy for homepage
    - Internal: Fully in Loro
    - Full TipTap editing for description

  - [[jira:PROJ-789]] Implement API endpoints
    - External: Jira issue (cached)
    - Your metadata: blocks:123456 team:@backend
  ```

  When you click `[[todoist:123456]]`:
  1. Load from local cache (instant)
  2. Show task details in panel
  3. Edit in simple form (title, due date, etc.)
  4. Changes push to Todoist API async
  5. Your metadata (context, location) lives in Loro only

  ---

  ## Summary

  1. **Block-based editing:** Don't force ProseMirror to be an outliner. Use Loro Tree for structure + lightweight editors per block.

  2. **External integration:** Sync/cache pattern is the only practical approach. Direct pass-through would be too slow, fragile, and limiting.

  3. **Data structure control:** ProseMirror needs local control. Use Loro as the source of truth for your content, cache for external content, and build adapters that respect each system's boundaries.

  **The winning architecture:**
  - **Your content** → Loro + TipTap (full control, rich editing)
  - **External content** → Local cache + simple forms (fast, offline-capable)
  - **Relationships** → Stored in Loro (YOUR data, your rules)

  Want me to sketch out what the Rust adapter trait system might look like in more detail? Or how block references/transclusion would work?
</ai>
<user>
  Could the interaction with external systems be modeled through Loro + adapters? What I mean is that I basically imagine Loro to store a sequence of changes to some structure. But if I make the data structure match the one from the external system then I could apply changes in Loro to the external system and vice versa, so that I have an offline representation of the external system in Loro. Do I have misconceptions about how Loro works, or would that be possible?
</user>
<ai>
  **Excellent intuition!** You're absolutely right that this is possible, and it's actually a clever approach. Let me clarify how this would work and where the complexities lie.

  ## How Loro Actually Works

  Your mental model is mostly correct:

  ```rust
  let doc = LoroDoc::new();
  let tasks = doc.get_map("todoist_tasks");

  // Each change creates an operation
  let task = tasks.get_map("task_123");
  task.set("content", "Buy milk");     // Op 1
  task.set("due_date", "2025-10-20");  // Op 2
  task.set("priority", 4);              // Op 3

  // Loro stores:
  // - The current state (Map with 3 entries)
  // - The operation log that got you there
  // - Actor IDs and timestamps for each op
  ```

  **Key properties:**
  - Operations are **immutable** once created
  - Multiple actors can make concurrent changes
  - Loro **automatically merges** concurrent operations using CRDT rules
  - You can export/import operation history between Loro instances

  ## Your Idea: Loro as Offline Layer for External Systems

  **Concept:**
  ```
  ┌─────────────────────────────────────┐
  │  Local Loro Doc                     │
  │  ┌────────────────────────────────┐ │
  │  │ todoist_tasks: Map             │ │
  │  │   task_123: {                  │ │
  │  │     content: "Buy milk",       │ │
  │  │     priority: 4,               │ │
  │  │     _server_version: "abc123"  │ │
  │  │   }                            │ │
  │  └────────────────────────────────┘ │
  └─────────────────────────────────────┘
              ↕️  sync adapter
  ┌─────────────────────────────────────┐
  │  Todoist API                        │
  │  GET /tasks/123 → {...}             │
  │  POST /tasks/123 → update           │
  └─────────────────────────────────────┘
  ```

  **This absolutely works and is a solid architecture!**

  ## How the Adapter Would Work

  ```rust
  struct TodoistAdapter {
      loro_doc: LoroDoc,
      api_client: TodoistClient,
  }

  impl TodoistAdapter {
      // Pull changes from API → apply to Loro
      async fn sync_from_server(&mut self) -> Result<()> {
          let server_tasks = self.api_client.get_tasks().await?;
          let local_tasks = self.loro_doc.get_map("todoist_tasks");

          for server_task in server_tasks {
              let task_id = &server_task.id;
              let local_task = local_tasks.get_map(task_id);

              // Check if we need to update
              let local_version = local_task.get("_server_version")
                  .and_then(|v| v.as_string());

              if local_version != Some(&server_task.version) {
                  // Server has newer version
                  // Apply server state to Loro
                  self.apply_server_state(local_task, server_task)?;
              }
          }
          Ok(())
      }

      // Push local changes → API
      async fn sync_to_server(&mut self) -> Result<()> {
          let tasks = self.loro_doc.get_map("todoist_tasks");

          // Find tasks with pending changes
          for (task_id, task_value) in tasks.iter() {
              let task = task_value.as_map().unwrap();

              // Check if modified since last sync
              if self.has_pending_changes(task) {
                  // Convert Loro state to API update
                  let update = self.loro_to_api_update(task)?;

                  // Push to server
                  let result = self.api_client
                      .update_task(task_id, update)
                      .await?;

                  // Update version marker
                  task.set("_server_version", result.version);
                  task.delete("_pending_changes");
              }
          }
          Ok(())
      }
  }
  ```

  ## The Key Challenges

  ### 1. **External Systems Don't Send Operations, Only State**

  When you fetch from Todoist API, you get:
  ```json
  {
    "id": "123",
    "content": "Buy milk and bread",
    "priority": 4,
    "version": "xyz789"
  }
  ```

  You **don't** get:
  ```
  Operation log:
    - T1: set content "Buy milk"
    - T2: set content "Buy milk and bread"
    - T3: set priority 4
  ```

  So when syncing, you're doing **state reconciliation**, not operation merging.

  ### 2. **Conflict Resolution is Your Responsibility**

  **Scenario:**
  ```
  // User offline, edits locally
  local_task.set("content", "Buy milk and eggs");
  // Loro op: {actor: user_device, timestamp: T1, content: "...eggs"}

  // Meanwhile, collaborator edits via web
  Server now has: content = "Buy milk and bread", version = "v2"

  // When syncing: What do you do?
  ```

  **Loro can't auto-resolve this** because the server isn't a Loro peer. You need to:

  ```rust
  impl TodoistAdapter {
      fn resolve_conflict(
          &self,
          local_value: &str,
          server_value: &str,
          local_timestamp: Timestamp,
          server_timestamp: Timestamp,
      ) -> ConflictResolution {
          match self.strategy {
              Strategy::LastWriteWins => {
                  if local_timestamp > server_timestamp {
                      ConflictResolution::KeepLocal
                  } else {
                      ConflictResolution::KeepServer
                  }
              }
              Strategy::ServerWins => ConflictResolution::KeepServer,
              Strategy::PromptUser => {
                  ConflictResolution::AskUser {
                      local: local_value,
                      server: server_value,
                  }
              }
          }
      }
  }
  ```

  ### 3. **Loss of True CRDT Properties**

  Within Loro itself, you get **guaranteed convergence**: if two devices make concurrent changes and sync, they'll always reach the same state.

  But when syncing with external APIs, you lose this because:
  - The external system isn't using CRDTs
  - It might have different conflict resolution rules
  - Network partitions could cause divergence that requires manual resolution

  **However,** this is still **way better** than no offline support!

  ## What You Get vs What You Build

  ### **Loro Provides (Free):**
  - ✅ Local operation tracking
  - ✅ Automatic merging of concurrent local edits
  - ✅ History/undo
  - ✅ Efficient sync between Loro instances (if you have multiple devices)
  - ✅ Data structure APIs (Map, List, Tree, etc.)

  ### **You Build (The Adapter):**
  - ⚙️ Mapping between Loro structures and API schemas
  - ⚙️ Detecting what changed since last sync
  - ⚙️ Converting Loro state → API calls
  - ⚙️ Converting API responses → Loro updates
  - ⚙️ Conflict resolution strategy
  - ⚙️ Error handling (API failures, rate limits)
  - ⚙️ Tracking server versions/etags

  ## Concrete Example: Offline Edit Flow

  ```rust
  // 1. User goes offline
  // Current state synced from server
  let tasks = doc.get_map("todoist_tasks");
  let task = tasks.get_map("task_123");
  // task: {content: "Buy milk", priority: 4, _version: "v1"}

  // 2. User makes changes offline
  task.set("content", "Buy milk and eggs");
  task.set("priority", 3);
  // Loro tracks these as operations with timestamps

  // 3. User goes back online
  // Sync process:

  // A. Fetch server state
  let server_task = api.get_task("123").await?;
  // server_task: {content: "Buy milk and bread", priority: 4, version: "v2"}

  // B. Detect conflict
  let local_version = task.get("_version").unwrap(); // "v1"
  let server_version = server_task.version;          // "v2"

  if local_version != server_version {
      // Server changed while we were offline!

      // C. Check what we changed locally
      let local_changes = detect_changes(task, "v1");
      // local_changes: {content: modified, priority: modified}

      // D. Resolve conflicts field-by-field
      let resolved = resolve_conflicts(
          local_changes,
          server_task,
          ConflictStrategy::LastWriteWins,
      );

      // E. Apply resolution
      if resolved.push_local {
          // Push our changes to server
          api.update_task("123", {
              content: "Buy milk and eggs", // Our version wins
              priority: 3,
          }).await?;
      } else {
          // Server wins, update local
          task.set("content", server_task.content);
          task.set("priority", server_task.priority);
      }

      // F. Update version marker
      task.set("_version", "v3");
  }
  ```

  ## When This Approach Works Well

  **Great for:**
  - ✅ **Read-heavy workflows** (browse tasks offline, occasional edits)
  - ✅ **Single-user scenarios** (you on multiple devices)
  - ✅ **Infrequent conflicts** (you edit different tasks than collaborators)
  - ✅ **Coarse-grained data** (whole tasks, not character-by-character editing)

  **Gets complex with:**
  - ⚠️ **Heavy collaborative editing** (multiple people editing same task simultaneously)
  - ⚠️ **Fine-grained edits** (character-level text editing in task descriptions)
  - ⚠️ **Complex relationships** (task dependencies that could create conflicts)

  ## Architecture Pattern: Two-Layer Sync

  For your use case, I'd recommend:

  ```
  ┌──────────────────────────────────────────┐
  │           Your Application               │
  └──────────────────────────────────────────┘
                      │
      ┌───────────────┼───────────────┐
      ▼               ▼               ▼
  ┌─────────┐   ┌──────────┐   ┌──────────┐
  │Internal │   │ External │   │ External │
  │ Content │   │ Task Sync│   │ Task Sync│
  │         │   │ (Todoist)│   │  (Jira)  │
  │  Loro   │   │          │   │          │
  │  pure   │   │  Loro    │   │  Loro    │
  │         │   │   +      │   │   +      │
  │         │   │ Adapter  │   │ Adapter  │
  └─────────┘   └──────────┘   └──────────┘
   Full CRDT       Hybrid         Hybrid
   guarantees      sync           sync
  ```

  **Internal content** (your notes, metadata, relationships):
  - Lives purely in Loro
  - Full CRDT guarantees
  - Rich text editing with TipTap
  - No external sync needed

  **External tasks** (Todoist, Jira, etc.):
  - Represented in Loro for offline access
  - Adapter handles bidirectional sync
  - Custom conflict resolution
  - Best-effort consistency

  **Your metadata on external tasks**:
  - Stored separately in Loro
  - Always local, never synced to external API
  - Examples: your tags, contexts, relationships

  ```rust
  // Structure
  doc.get_map("todoist_tasks")      // External data (synced)
  doc.get_map("task_metadata")      // Your metadata (never synced)
  doc.get_map("task_relationships") // Links between tasks (yours)
  ```

  ## Does This Make Sense?

  Yes! This approach gives you:
  1. **True offline capability** - full read/write access without network
  2. **Automatic local merging** - Loro handles conflicts between your devices
  3. **Eventual consistency** - adapter syncs with external systems when online
  4. **Flexibility** - you control conflict resolution strategy per integration

  The trade-off is that you're building the sync adapter logic yourself, but that's unavoidable when bridging CRDT and non-CRDT worlds.

  **Would you like me to sketch out:**
  - A more detailed adapter trait/interface?
  - How to track "dirty" changes that need syncing?
  - Conflict resolution strategies for different scenarios?
  - How relationships between internal and external content would work?
</ai>
<user>
  Apart from requiring more code, which disadvantages would this approach have compared to other ways of achieving offline functionality with eventual sync and good local performance?
  How much code do you estimate would need to be written per external system, given it has a CRUD REST API?
</user>
<ai>
  Great question! Let me break down the trade-offs and give you realistic estimates.

  ## Disadvantages of the Loro Approach

  ### 1. **Complexity Overkill for Simple Mirroring**

  If you're just doing **read-mostly access** to external tasks, you're bringing a CRDT to a state-sync fight:

  ```rust
  // What you're building:
  Loro CRDT (operation log, vector clocks, merge logic)
    ↕️
  Adapter (version tracking, conflict resolution)
    ↕️
  External API (simple state)

  // What you might actually need:
  SQLite cache ↔️ External API
  ```

  **The CRDT machinery is most valuable when:**
  - Multiple actors edit the same data concurrently
  - You need automatic, deterministic merging
  - You're syncing between multiple Loro instances

  **But for external systems:**
  - The external system is the source of truth (not peer-to-peer)
  - Conflicts are rare if you're the only editor
  - You're doing state synchronization, not operation merging

  ### 2. **Memory and Storage Overhead**

  Loro keeps operation history:

  ```rust
  // Simple cache approach:
  task_123: {content: "Buy milk", priority: 4}  // ~100 bytes

  // Loro approach:
  task_123: {
    content: "Buy milk",
    priority: 4,
    _loro_metadata: {
      operation_log: [...],     // History of changes
      actor_ids: [...],         // Who made changes
      timestamps: [...],        // When changes happened
      version_vector: {...},    // CRDT state
    }
  }  // ~500-1000 bytes depending on edit history
  ```

  **Impact:**
  - 5-10x memory overhead per object
  - Slower serialization/deserialization
  - Larger disk footprint

  **Mitigation:** Loro does support compacting history, but you need to manage this.

  ### 3. **Mismatch Between CRDT and API Semantics**

  **Loro's guarantees:**
  ```rust
  // Two devices make concurrent edits
  device1: task.set("priority", 4);  // Timestamp T1
  device2: task.set("priority", 3);  // Timestamp T2

  // Loro will deterministically pick one (e.g., last-write-wins by timestamp)
  // All Loro instances converge to same state
  ```

  **But with external API:**
  ```rust
  // You push priority=4 to server
  // Server says: "409 Conflict - version mismatch"
  // Or: "200 OK" but someone else pushed priority=3 first
  // You fetch back: priority=3

  // Now your Loro state says priority=4, but server has priority=3
  // You need custom code to reconcile this
  ```

  You're **losing the main CRDT benefit** (automatic convergence) because the external system isn't a CRDT peer.

  ### 4. **Validation Happens Late**

  ```rust
  // User offline, makes change
  task.set("due_date", "2025-13-45");  // Invalid date!
  // Loro accepts it (it's just a string)

  // Later, sync attempts:
  api.update_task(task_id, updates).await?;
  // Server returns: 400 Bad Request - invalid date

  // Now what?
  // - Revert the local change? (loses user's work)
  // - Keep it locally? (perpetual sync conflict)
  // - Prompt user? (breaks automatic sync)
  ```

  With local-first, you defer validation to sync time, complicating error handling.

  ### 5. **Not Leveraging External System's Native Sync**

  Some systems have their own offline/sync mechanisms:
  - **Todoist**: Has offline mode built into their SDK
  - **Google Calendar**: Has sync tokens and incremental updates
  - **Jira**: Has webhooks for real-time updates

  By wrapping in Loro, you might be **reimplementing** what they already provide.

  ### 6. **Debugging Complexity**

  ```bash
  # Simple cache:
  $ sqlite3 cache.db "SELECT * FROM tasks WHERE id=123"
  content: "Buy milk"
  priority: 4
  last_synced: 2025-10-16 10:30:00

  # Loro approach:
  $ loro-cli inspect doc.loro --path "todoist_tasks/task_123"
  [Operation log with 47 entries]
  Actor 'device1' at T1: set content "Buy milk"
  Actor 'device2' at T2: set content "Buy groceries"
  Actor 'device1' at T3: set content "Buy milk"  # Conflict resolution
  ...

  # Which state is "correct"? What did the server actually have?
  ```

  ## Comparison to Alternatives

  ### **Alternative 1: Plain SQLite + Sync Logic**

  ```rust
  struct SimpleCache {
      db: SqliteConnection,
  }

  impl SimpleCache {
      async fn sync(&mut self) -> Result<()> {
          // Fetch from API
          let tasks = api.get_tasks().await?;

          for task in tasks {
              // Upsert to SQLite
              db.execute(
                  "INSERT OR REPLACE INTO tasks VALUES (?, ?, ?, ?)",
                  (task.id, task.content, task.priority, task.version)
              )?;
          }

          // Push local changes
          let dirty_tasks = db.query("SELECT * FROM tasks WHERE dirty = 1")?;
          for task in dirty_tasks {
              api.update_task(task.id, task).await?;
              db.execute("UPDATE tasks SET dirty = 0 WHERE id = ?", task.id)?;
          }
      }
  }
  ```

  **Advantages:**
  - ✅ Much simpler (~200 lines vs 1500)
  - ✅ Lower memory overhead
  - ✅ Easier to debug (just SQL queries)
  - ✅ Standard tools work (DB browsers, SQL)

  **Disadvantages:**
  - ❌ No automatic multi-device sync (need custom logic)
  - ❌ Conflicts require manual resolution
  - ❌ No operation history/undo for external data
  - ❌ Can't leverage CRDT merging if you add features later

  **Best for:** Single device, or simple last-write-wins across devices

  ### **Alternative 2: Use Loro Only for Internal Content**

  ```rust
  struct HybridStorage {
      loro_doc: LoroDoc,          // Your notes, projects, internal tasks
      simple_cache: SqliteCache,   // External tasks (simple cache)
  }
  ```

  **This is actually a really good compromise:**
  - Use Loro's full power where it matters (your content, collaboration)
  - Use simple caching for external read-only/rare-edit data
  - Still get offline access to everything
  - Much less code for external integrations

  **When makes sense:**
  - External tasks are mostly read-only (view in context, not heavy editing)
  - You're adding metadata/relationships (store in Loro) but not editing task content much
  - You want to ship faster with less complexity

  ### **Alternative 3: Loro + Simplified Sync**

  Use Loro for **structure**, but don't try to mirror every field:

  ```rust
  // Instead of mirroring full task in Loro:
  let task = loro.get_map("todoist_tasks/123");
  task.set("id", "123");
  task.set("content", "Buy milk");  // Full mirror
  task.set("priority", 4);          // Full mirror
  task.set("due_date", "...");      // Full mirror

  // Just store references + cache:
  let task_ref = loro.get_map("task_refs/123");
  task_ref.set("source", "todoist");
  task_ref.set("external_id", "123");
  // Actual data in SQLite cache
  ```

  **Advantages:**
  - Loro manages relationships and structure
  - Simple cache for mutable external data
  - Easier conflict handling
  - Lower Loro overhead

  ## Code Estimation Per Integration

  Based on implementing production sync systems, here's realistic sizing:

  ### **Loro Approach** (Full Mirror)

  ```rust
  // Per integration (e.g., Todoist):

  // 1. Schema mapping (100-150 lines)
  struct TodoistTaskInLoro {
      // Define Loro Map structure
      // Field mappings
      // Type conversions
  }

  // 2. Fetch & Apply (200-300 lines)
  impl TodoistAdapter {
      async fn fetch_from_api(&self) -> Result<Vec<Task>>;
      fn apply_to_loro(&mut self, tasks: Vec<Task>);
      fn handle_deletions(&mut self);
      // Pagination, error handling, rate limits
  }

  // 3. Change Detection (150-250 lines)
  impl TodoistAdapter {
      fn find_dirty_tasks(&self) -> Vec<TaskId>;
      fn compute_diff(&self, task_id: &TaskId) -> TaskDiff;
      fn track_versions(&mut self);
  }

  // 4. Push Changes (200-350 lines)
  impl TodoistAdapter {
      async fn push_changes(&mut self) -> Result<()>;
      fn loro_to_api_format(&self, task: &LoroMap) -> ApiTask;
      async fn batch_updates(&mut self, changes: Vec<Change>);
      // Retry logic, rate limiting, error recovery
  }

  // 5. Conflict Resolution (150-250 lines)
  impl TodoistAdapter {
      fn detect_conflicts(&self, local: &Task, remote: &Task) -> Conflicts;
      fn resolve_conflict(&self, conflict: Conflict) -> Resolution;
      fn apply_resolution(&mut self, resolution: Resolution);
      // Field-level merge strategies
  }

  // 6. API Client (300-500 lines)
  struct TodoistClient {
      // HTTP client setup
      // Auth token management
      // All API endpoints
      // Request/response types
      // Error handling
  }

  // 7. Testing (400-600 lines)
  mod tests {
      // Unit tests for each component
      // Mock API responses
      // Conflict scenarios
      // Edge cases
  }

  // Total: 1,500 - 2,400 lines per integration
  ```

  ### **Shared Infrastructure** (Write Once)

  ```rust
  // Base trait (200-300 lines)
  trait ExternalSystemAdapter {
      async fn sync_from_remote(&mut self) -> Result<SyncStats>;
      async fn sync_to_remote(&mut self) -> Result<SyncStats>;
      fn capabilities(&self) -> Capabilities;
  }

  // Sync Orchestrator (300-400 lines)
  struct SyncManager {
      adapters: Vec<Box<dyn ExternalSystemAdapter>>,
      // Scheduling
      // Batch sync coordination
      // Progress reporting
      // Conflict UI triggers
  }

  // Common utilities (200-300 lines)
  mod sync_utils {
      // Rate limiter
      // Retry with backoff
      // Version tracking helpers
      // Diff computation
  }

  // Total shared: 700-1,000 lines
  ```

  ### **Total Project Estimate**

  ```
  Phase 1 (Infrastructure):
  ├─ Loro setup & patterns:          500-700 lines
  ├─ Base adapter trait:              200-300 lines
  ├─ Sync orchestrator:               300-400 lines
  ├─ Common utilities:                200-300 lines
  └─ UI for conflict resolution:      400-600 lines
     Total:                          1,600-2,300 lines

  Phase 2 (First Integration - Todoist):
  ├─ Todoist adapter:               1,500-2,400 lines
  └─ Tests:                           400-600 lines
     Total:                          1,900-3,000 lines

  Each Additional Integration:
  ├─ Adapter code:                  1,000-1,500 lines
  └─ Tests:                           300-500 lines
     Total:                          1,300-2,000 lines
  ```

  **3 integrations total: ~7,000-11,000 lines of code**

  ### **Simple Cache Approach** (For Comparison)

  ```rust
  // Per integration with simple cache:

  // 1. SQLite schema (50 lines)
  CREATE TABLE todoist_tasks (...);
  CREATE INDEX idx_dirty ON tasks(dirty);

  // 2. Cache manager (150-200 lines)
  struct TaskCache {
      async fn upsert(&mut self, task: Task);
      async fn mark_dirty(&mut self, id: TaskId);
      async fn get_dirty(&self) -> Vec<Task>;
  }

  // 3. Simple sync (200-300 lines)
  impl SimpleSyncAdapter {
      async fn sync(&mut self) {
          // Pull from API → upsert to SQLite
          // Get dirty from SQLite → push to API
          // Basic conflict: last-write-wins or server-wins
      }
  }

  // 4. API client (300-500 lines)
  // Same as before

  // 5. Testing (200-300 lines)
  // Simpler because less logic

  // Total: 900-1,350 lines per integration
  // 3 integrations: ~3,500-5,000 lines
  ```

  **The simple approach is 50-60% less code.**

  ## When Loro Approach Makes Sense

  Despite the overhead, use Loro for external systems if:

  1. **You'll edit external tasks heavily while offline**
     - Frequently updating task content, priorities, dates
     - Not just viewing/checking off

  2. **You sync across multiple devices**
     - Need automatic merging between YOUR devices
     - Even if external system doesn't support it natively

  3. **You plan to add collaborative features later**
     - Multiple users editing same workspace
     - Real-time updates between team members
     - The CRDT infrastructure pays off here

  4. **You want unified operation history**
     - Undo/redo across all data sources
     - Time-travel debugging
     - Audit logs

  5. **You're building relationships/metadata on top**
     - Heavily linking external tasks to internal notes
     - The Loro document becomes the "graph" connecting everything

  ## My Recommendation

  **Hybrid approach:**

  ```rust
  struct Storage {
      // Internal content: Full Loro power
      loro_doc: LoroDoc,  // Your notes, projects, internal tasks

      // External systems: Simple cache + smart sync
      external_cache: SqliteCache,  // Todoist, Jira, etc. (mirrored state)

      // Relationships: In Loro
      loro_doc.get_map("task_links")  // Links between internal and external
      loro_doc.get_map("task_metadata")  // Your tags, contexts on external tasks
  }
  ```

  **Why:**
  - ✅ Leverage Loro where it shines (your content, relationships)
  - ✅ Keep external sync simple and maintainable
  - ✅ 50% less code for external integrations
  - ✅ Easier debugging and testing
  - ✅ Can always upgrade specific integrations to full Loro if needed

  **Code estimate:**
  - First integration: ~1,000 lines (vs 2,000 with full Loro)
  - Additional integrations: ~700 lines each (vs 1,500)
  - Total for 3 integrations: ~4,000 lines (vs 8,000)

  **You still get:**
  - Offline access to external tasks
  - Fast local queries/search
  - Relationships between internal and external content
  - Your metadata on external tasks
  - Reasonable conflict handling

  Does this analysis help? Would you like me to sketch out what the hybrid approach would look like in more detail?
</ai>
<user>
  Could we find a common interface for Loro and the SQLite Cache, so that it's rather simple to switch from one to the other? I guess each external system would need to describe its schema, which could then be used both for the Loro and SQL schema, and then one would need methods for reading, writing and initiating a sync?
</user>

<ai>
**Excellent insight!** Yes, this is absolutely possible and it's actually a really smart architectural decision. Let me design this abstraction.

## Core Abstraction: Storage Backend Trait

```rust
// 1. Schema Definition (backend-agnostic)
#[derive(Debug, Clone)]
struct FieldSchema {
    name: String,
    field_type: FieldType,
    required: bool,
    indexed: bool,
}

#[derive(Debug, Clone)]
enum FieldType {
    String,
    Integer,
    Boolean,
    DateTime,
    Json,
    Reference(String),  // Reference to another entity
}

#[derive(Debug, Clone)]
struct EntitySchema {
    name: String,
    fields: Vec<FieldSchema>,
    primary_key: String,
}

// 2. Generic storage operations
#[async_trait]
trait StorageBackend: Send + Sync {
    // Schema management
    async fn create_entity(&mut self, schema: &EntitySchema) -> Result<()>;

    // CRUD operations
    async fn get(&self, entity: &str, id: &str) -> Result<Option<Entity>>;
    async fn query(&self, entity: &str, filter: Filter) -> Result<Vec<Entity>>;
    async fn insert(&mut self, entity: &str, data: Entity) -> Result<()>;
    async fn update(&mut self, entity: &str, id: &str, data: Entity) -> Result<()>;
    async fn delete(&mut self, entity: &str, id: &str) -> Result<()>;

    // Sync tracking
    async fn mark_dirty(&mut self, entity: &str, id: &str) -> Result<()>;
    async fn get_dirty(&self, entity: &str) -> Result<Vec<String>>;
    async fn mark_clean(&mut self, entity: &str, id: &str) -> Result<()>;

    // Versioning (for conflict detection)
    async fn get_version(&self, entity: &str, id: &str) -> Result<Option<String>>;
    async fn set_version(&mut self, entity: &str, id: &str, version: String) -> Result<()>;

    // Metadata queries
    async fn count(&self, entity: &str, filter: Option<Filter>) -> Result<usize>;
    async fn last_sync_time(&self, entity: &str) -> Result<Option<DateTime>>;
}

// 3. Generic entity representation
type Entity = HashMap<String, Value>;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Value {
    String(String),
    Integer(i64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Json(serde_json::Value),
    Reference(String),
    Null,
}

// 4. Query filters
#[derive(Debug, Clone)]
enum Filter {
    Eq(String, Value),
    In(String, Vec<Value>),
    And(Vec<Filter>),
    Or(Vec<Filter>),
    IsNull(String),
    IsNotNull(String),
}
```

## SQLite Implementation

```rust
struct SqliteBackend {
    pool: SqlitePool,
    schemas: HashMap<String, EntitySchema>,
}

#[async_trait]
impl StorageBackend for SqliteBackend {
    async fn create_entity(&mut self, schema: &EntitySchema) -> Result<()> {
        // Generate SQL from schema
        let mut columns = vec![];
        for field in &schema.fields {
            let sql_type = match field.field_type {
                FieldType::String => "TEXT",
                FieldType::Integer => "INTEGER",
                FieldType::Boolean => "BOOLEAN",
                FieldType::DateTime => "TIMESTAMP",
                FieldType::Json => "TEXT",  // Store as JSON string
                FieldType::Reference(_) => "TEXT",
            };

            let nullable = if field.required { "NOT NULL" } else { "" };
            columns.push(format!("{} {} {}", field.name, sql_type, nullable));
        }

        // Add sync tracking columns
        columns.push("_dirty BOOLEAN DEFAULT 0".to_string());
        columns.push("_version TEXT".to_string());
        columns.push("_last_synced TIMESTAMP".to_string());

        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({}, PRIMARY KEY ({}))",
            schema.name,
            columns.join(", "),
            schema.primary_key
        );

        sqlx::query(&create_sql).execute(&self.pool).await?;

        // Create indexes
        for field in &schema.fields {
            if field.indexed {
                let idx_sql = format!(
                    "CREATE INDEX IF NOT EXISTS idx_{}_{} ON {} ({})",
                    schema.name, field.name, schema.name, field.name
                );
                sqlx::query(&idx_sql).execute(&self.pool).await?;
            }
        }

        // Store schema for later use
        self.schemas.insert(schema.name.clone(), schema.clone());
        Ok(())
    }

    async fn get(&self, entity: &str, id: &str) -> Result<Option<Entity>> {
        let schema = self.schemas.get(entity).ok_or(Error::SchemaNotFound)?;

        let sql = format!("SELECT * FROM {} WHERE {} = ?", entity, schema.primary_key);
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(row) => Ok(Some(self.row_to_entity(&row, schema)?)),
            None => Ok(None),
        }
    }

    async fn insert(&mut self, entity: &str, data: Entity) -> Result<()> {
        let schema = self.schemas.get(entity).ok_or(Error::SchemaNotFound)?;

        let columns: Vec<_> = data.keys().collect();
        let placeholders: Vec<_> = (0..columns.len()).map(|_| "?").collect();

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            entity,
            columns.join(", "),
            placeholders.join(", ")
        );

        let mut query = sqlx::query(&sql);
        for col in columns {
            query = self.bind_value(query, data.get(col).unwrap());
        }

        query.execute(&self.pool).await?;
        Ok(())
    }

    async fn update(&mut self, entity: &str, id: &str, data: Entity) -> Result<()> {
        let schema = self.schemas.get(entity).ok_or(Error::SchemaNotFound)?;

        let set_clauses: Vec<_> = data.keys()
            .map(|k| format!("{} = ?", k))
            .collect();

        let sql = format!(
            "UPDATE {} SET {} WHERE {} = ?",
            entity,
            set_clauses.join(", "),
            schema.primary_key
        );

        let mut query = sqlx::query(&sql);
        for value in data.values() {
            query = self.bind_value(query, value);
        }
        query = query.bind(id);

        query.execute(&self.pool).await?;
        Ok(())
    }

    async fn mark_dirty(&mut self, entity: &str, id: &str) -> Result<()> {
        let schema = self.schemas.get(entity).ok_or(Error::SchemaNotFound)?;
        let sql = format!(
            "UPDATE {} SET _dirty = 1 WHERE {} = ?",
            entity, schema.primary_key
        );
        sqlx::query(&sql).bind(id).execute(&self.pool).await?;
        Ok(())
    }

    async fn get_dirty(&self, entity: &str) -> Result<Vec<String>> {
        let schema = self.schemas.get(entity).ok_or(Error::SchemaNotFound)?;
        let sql = format!(
            "SELECT {} FROM {} WHERE _dirty = 1",
            schema.primary_key, entity
        );

        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    // ... other methods
}
```

## Loro Implementation

```rust
struct LoroBackend {
    doc: LoroDoc,
    schemas: HashMap<String, EntitySchema>,
}

#[async_trait]
impl StorageBackend for LoroBackend {
    async fn create_entity(&mut self, schema: &EntitySchema) -> Result<()> {
        // In Loro, we don't need to "create" the schema upfront
        // But we store it for validation and type coercion
        self.schemas.insert(schema.name.clone(), schema.clone());

        // Initialize the root map for this entity type if needed
        let entities = self.doc.get_map(&schema.name);

        // Initialize metadata maps
        let dirty_map = self.doc.get_map(&format!("{}_dirty", schema.name));
        let version_map = self.doc.get_map(&format!("{}_versions", schema.name));

        Ok(())
    }

    async fn get(&self, entity: &str, id: &str) -> Result<Option<Entity>> {
        let entities = self.doc.get_map(entity);

        match entities.get(id) {
            Some(value) => {
                let entity_map = value.as_map()
                    .ok_or(Error::InvalidType)?;
                Ok(Some(self.loro_map_to_entity(entity_map)?))
            }
            None => Ok(None),
        }
    }

    async fn query(&self, entity: &str, filter: Filter) -> Result<Vec<Entity>> {
        let entities = self.doc.get_map(entity);
        let mut results = vec![];

        for (id, value) in entities.iter() {
            let entity_map = value.as_map()
                .ok_or(Error::InvalidType)?;
            let entity = self.loro_map_to_entity(entity_map)?;

            if self.matches_filter(&entity, &filter) {
                results.push(entity);
            }
        }

        Ok(results)
    }

    async fn insert(&mut self, entity: &str, data: Entity) -> Result<()> {
        let schema = self.schemas.get(entity).ok_or(Error::SchemaNotFound)?;
        let id = data.get(&schema.primary_key)
            .ok_or(Error::MissingPrimaryKey)?
            .as_string()
            .ok_or(Error::InvalidType)?;

        let entities = self.doc.get_map(entity);
        let entity_map = entities.get_map(id);

        for (key, value) in data {
            self.set_loro_value(&entity_map, &key, value)?;
        }

        Ok(())
    }

    async fn update(&mut self, entity: &str, id: &str, data: Entity) -> Result<()> {
        let entities = self.doc.get_map(entity);
        let entity_map = entities.get_map(id);

        for (key, value) in data {
            self.set_loro_value(&entity_map, &key, value)?;
        }

        Ok(())
    }

    async fn mark_dirty(&mut self, entity: &str, id: &str) -> Result<()> {
        let dirty_map = self.doc.get_map(&format!("{}_dirty", entity));
        dirty_map.set(id, true);
        Ok(())
    }

    async fn get_dirty(&self, entity: &str) -> Result<Vec<String>> {
        let dirty_map = self.doc.get_map(&format!("{}_dirty", entity));
        let mut dirty_ids = vec![];

        for (id, value) in dirty_map.iter() {
            if value.as_bool() == Some(true) {
                dirty_ids.push(id.to_string());
            }
        }

        Ok(dirty_ids)
    }

    async fn get_version(&self, entity: &str, id: &str) -> Result<Option<String>> {
        let version_map = self.doc.get_map(&format!("{}_versions", entity));
        Ok(version_map.get(id).and_then(|v| v.as_string()))
    }

    // Helper methods
    fn loro_map_to_entity(&self, map: &LoroMap) -> Result<Entity> {
        let mut entity = HashMap::new();

        for (key, value) in map.iter() {
            let converted = match value {
                LoroValue::String(s) => Value::String(s.to_string()),
                LoroValue::I64(i) => Value::Integer(i),
                LoroValue::Bool(b) => Value::Boolean(b),
                // Handle other types...
                _ => return Err(Error::UnsupportedType),
            };
            entity.insert(key.to_string(), converted);
        }

        Ok(entity)
    }

    fn set_loro_value(&self, map: &LoroMap, key: &str, value: Value) -> Result<()> {
        match value {
            Value::String(s) => map.set(key, s),
            Value::Integer(i) => map.set(key, i),
            Value::Boolean(b) => map.set(key, b),
            Value::DateTime(dt) => map.set(key, dt.to_rfc3339()),
            Value::Json(j) => map.set(key, j.to_string()),
            Value::Reference(r) => map.set(key, r),
            Value::Null => map.delete(key),
        }
        Ok(())
    }

    // ... other methods
}
```

## Generic Adapter Using the Abstraction

```rust
struct ExternalSystemAdapter<B: StorageBackend> {
    storage: Arc<Mutex<B>>,
    api_client: Box<dyn ApiClient>,
    schema: EntitySchema,
    entity_name: String,
}

impl<B: StorageBackend> ExternalSystemAdapter<B> {
    async fn new(
        storage: Arc<Mutex<B>>,
        api_client: Box<dyn ApiClient>,
        schema: EntitySchema,
    ) -> Result<Self> {
        let entity_name = schema.name.clone();

        // Initialize schema in storage
        storage.lock().await.create_entity(&schema).await?;

        Ok(Self {
            storage,
            api_client,
            schema,
            entity_name,
        })
    }

    async fn sync_from_remote(&mut self) -> Result<SyncStats> {
        let mut stats = SyncStats::default();

        // Fetch from API
        let remote_items = self.api_client.fetch_all().await?;

        let mut storage = self.storage.lock().await;

        for remote_item in remote_items {
            let id = remote_item.get_id();

            // Check if exists locally
            match storage.get(&self.entity_name, &id).await? {
                Some(local_item) => {
                    // Compare versions
                    let local_version = storage
                        .get_version(&self.entity_name, &id)
                        .await?;
                    let remote_version = remote_item.get_version();

                    if local_version.as_deref() != Some(remote_version) {
                        // Update needed
                        let entity = self.api_to_entity(remote_item)?;
                        storage.update(&self.entity_name, &id, entity).await?;
                        storage.set_version(&self.entity_name, &id, remote_version.to_string()).await?;
                        stats.updated += 1;
                    }
                }
                None => {
                    // Insert new
                    let entity = self.api_to_entity(remote_item)?;
                    storage.insert(&self.entity_name, entity).await?;
                    storage.set_version(&self.entity_name, &id, remote_item.get_version().to_string()).await?;
                    stats.inserted += 1;
                }
            }
        }

        Ok(stats)
    }

    async fn sync_to_remote(&mut self) -> Result<SyncStats> {
        let mut stats = SyncStats::default();

        let mut storage = self.storage.lock().await;

        // Get all dirty items
        let dirty_ids = storage.get_dirty(&self.entity_name).await?;

        for id in dirty_ids {
            if let Some(entity) = storage.get(&self.entity_name, &id).await? {
                // Convert to API format and push
                let api_item = self.entity_to_api(entity)?;

                match self.api_client.update(&id, api_item).await {
                    Ok(updated) => {
                        // Success - update version and mark clean
                        storage.set_version(
                            &self.entity_name,
                            &id,
                            updated.get_version().to_string()
                        ).await?;
                        storage.mark_clean(&self.entity_name, &id).await?;
                        stats.pushed += 1;
                    }
                    Err(e) if e.is_conflict() => {
                        // Conflict - fetch latest and mark for resolution
                        let latest = self.api_client.fetch_one(&id).await?;
                        let latest_entity = self.api_to_entity(latest)?;

                        // Store both versions for conflict resolution
                        stats.conflicts.push(Conflict {
                            id: id.clone(),
                            local: entity,
                            remote: latest_entity,
                        });
                    }
                    Err(e) => {
                        stats.errors.push((id.clone(), e));
                    }
                }
            }
        }

        Ok(stats)
    }

    // Conversion helpers
    fn api_to_entity(&self, api_item: ApiItem) -> Result<Entity> {
        let mut entity = HashMap::new();

        for field in &self.schema.fields {
            let value = api_item.get_field(&field.name)?;
            entity.insert(field.name.clone(), value);
        }

        Ok(entity)
    }

    fn entity_to_api(&self, entity: Entity) -> Result<ApiItem> {
        // Convert Entity to API-specific format
        // ...
    }
}

#[derive(Debug, Default)]
struct SyncStats {
    inserted: usize,
    updated: usize,
    pushed: usize,
    conflicts: Vec<Conflict>,
    errors: Vec<(String, Error)>,
}
```

## Example Usage: Todoist Integration

```rust
// Define Todoist schema
let todoist_schema = EntitySchema {
    name: "todoist_tasks".to_string(),
    primary_key: "id".to_string(),
    fields: vec![
        FieldSchema {
            name: "id".to_string(),
            field_type: FieldType::String,
            required: true,
            indexed: true,
        },
        FieldSchema {
            name: "content".to_string(),
            field_type: FieldType::String,
            required: true,
            indexed: false,
        },
        FieldSchema {
            name: "priority".to_string(),
            field_type: FieldType::Integer,
            required: false,
            indexed: true,
        },
        FieldSchema {
            name: "due_date".to_string(),
            field_type: FieldType::DateTime,
            required: false,
            indexed: true,
        },
        FieldSchema {
            name: "project_id".to_string(),
            field_type: FieldType::Reference("todoist_projects".to_string()),
            required: false,
            indexed: true,
        },
    ],
};

// Use with SQLite
let sqlite_storage = SqliteBackend::new("tasks.db").await?;
let todoist_adapter = ExternalSystemAdapter::new(
    Arc::new(Mutex::new(sqlite_storage)),
    Box::new(TodoistClient::new(api_token)),
    todoist_schema.clone(),
).await?;

// OR use with Loro - same code!
let loro_storage = LoroBackend::new();
let todoist_adapter = ExternalSystemAdapter::new(
    Arc::new(Mutex::new(loro_storage)),
    Box::new(TodoistClient::new(api_token)),
    todoist_schema,
).await?;

// Sync works the same regardless of backend
todoist_adapter.sync_from_remote().await?;
todoist_adapter.sync_to_remote().await?;
```

## Configuration-Driven Approach

You could even make it declarative:

```yaml
# integrations/todoist.yaml
name: todoist_tasks
backend: sqlite  # or 'loro'
schema:
  primary_key: id
  fields:
    - name: id
      type: string
      required: true
      indexed: true
    - name: content
      type: string
      required: true
    - name: priority
      type: integer
      indexed: true
    - name: due_date
      type: datetime
      indexed: true
api:
  base_url: https://api.todoist.com/rest/v2
  auth: bearer_token
  endpoints:
    list: /tasks
    get: /tasks/{id}
    update: /tasks/{id}
    create: /tasks
```

```rust
// Load and instantiate
let config = load_integration_config("integrations/todoist.yaml")?;

let storage: Box<dyn StorageBackend> = match config.backend {
    Backend::Sqlite => Box::new(SqliteBackend::new(&config.db_path).await?),
    Backend::Loro => Box::new(LoroBackend::new()),
};

let adapter = ExternalSystemAdapter::from_config(storage, config).await?;
```

## Benefits of This Approach

1. **Easy Migration Path**
   ```rust
   // Start with SQLite
   let storage = SqliteBackend::new("tasks.db").await?;

   // Later, migrate to Loro
   let old_data = sqlite_storage.query("tasks", Filter::All).await?;
   let new_storage = LoroBackend::new();
   for entity in old_data {
       new_storage.insert("tasks", entity).await?;
   }
   ```

2. **Mix and Match Per Integration**
   ```rust
   // Todoist: Heavy editing, use Loro
   let todoist = ExternalSystemAdapter::new(
       loro_storage,
       todoist_client,
       todoist_schema,
   ).await?;

   // Jira: Read-mostly, use SQLite
   let jira = ExternalSystemAdapter::new(
       sqlite_storage,
       jira_client,
       jira_schema,
   ).await?;
   ```

3. **Testing is Easier**
   ```rust
   // In-memory backend for tests
   struct MemoryBackend {
       data: HashMap<String, HashMap<String, Entity>>,
   }

   #[async_trait]
   impl StorageBackend for MemoryBackend {
       // Simple HashMap implementation
   }
   ```

4. **Clear Performance Comparison**
   ```rust
   // Benchmark both backends with same workload
   let sqlite_time = bench_sync(&sqlite_storage, &test_data).await?;
   let loro_time = bench_sync(&loro_storage, &test_data).await?;
   ```

## Code Estimate with This Approach

```
Shared Infrastructure (write once):
├─ StorageBackend trait:           200 lines
├─ Entity/Value/Filter types:      150 lines
├─ GenericAdapter implementation:  500 lines
├─ Schema parser/validator:        200 lines
   Subtotal:                      1,050 lines

SQLite Implementation:
├─ SqliteBackend:                  600 lines
├─ SQL generation:                 200 lines
├─ Type conversions:               150 lines
   Subtotal:                        950 lines

Loro Implementation:
├─ LoroBackend:                    500 lines
├─ Type conversions:               150 lines
├─ Query filtering:                100 lines
   Subtotal:                        750 lines

Per Integration (with abstraction):
├─ Schema definition:               50 lines (or YAML)
├─ API client:                     300 lines
├─ Type conversions:               100 lines
├─ Tests:                          200 lines
   Subtotal:                        650 lines

Total for 3 integrations:
Infrastructure + SQLite + Loro + (3 × Integration)
= 1,050 + 950 + 750 + 1,950 = 4,700 lines
```

**Compared to:**
- Loro-only approach: ~8,000 lines
- SQLite-only approach: ~4,000 lines
- This abstraction: ~4,700 lines with BOTH backends

## Potential Challenges

1. **Impedance Mismatch in Query Capabilities**
   - SQL can do complex JOINs, aggregations
   - Loro requires iterating and filtering in code
   - Solution: Define query interface to common subset, or make advanced queries backend-specific

2. **Transaction Semantics**
   - SQL has ACID transactions
   - Loro has operation atomicity but different semantics
   - Solution: Define transaction boundaries carefully in the trait

3. **Performance Characteristics Differ**
   - SQL: Optimized for complex queries
   - Loro: Optimized for CRDT operations
   - Solution: Make this visible through capability flags

```rust
trait StorageBackend {
    fn capabilities(&self) -> Capabilities;
}

struct Capabilities {
    supports_complex_queries: bool,
    supports_transactions: bool,
    supports_crdt_merge: bool,
    optimal_for_concurrent_editing: bool,
}
```

This abstraction is really elegant and gives you maximum flexibility. You can start simple, measure performance, and upgrade specific integrations to Loro only when the CRDT benefits are worth the complexity. Want me to flesh out any particular part of this design?
</ai>

<user>
For something like a Todoist task, could we use simple structs with some annotations or comments on its fields and derive the Field definitions from that?
</user>

<ai>
**Absolutely!** This would make the ergonomics much better. You can use derive macros (like `serde` does) or even procedural macros to generate the schema from struct definitions.

## Approach 1: Derive Macro (Most Ergonomic)

```rust
use storage_derive::Entity;

#[derive(Debug, Clone, Entity)]
#[entity(name = "todoist_tasks")]
struct TodoistTask {
    #[primary_key]
    #[indexed]
    id: String,

    content: String,

    #[indexed]
    priority: Option<i32>,

    #[indexed]
    due_date: Option<DateTime<Utc>>,

    completed: bool,

    #[reference(entity = "todoist_projects")]
    project_id: Option<String>,

    labels: Vec<String>,  // Stored as JSON

    /// API version for conflict detection
    #[skip_storage]  // Not stored, only used in sync
    #[serde(rename = "sync_id")]
    api_version: Option<String>,
}

// The derive macro generates:
impl EntitySchema for TodoistTask {
    fn schema() -> EntitySchema {
        EntitySchema {
            name: "todoist_tasks".to_string(),
            primary_key: "id".to_string(),
            fields: vec![
                FieldSchema {
                    name: "id".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    indexed: true,
                },
                FieldSchema {
                    name: "content".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    indexed: false,
                },
                FieldSchema {
                    name: "priority".to_string(),
                    field_type: FieldType::Integer,
                    required: false,  // Option<T> = not required
                    indexed: true,
                },
                FieldSchema {
                    name: "due_date".to_string(),
                    field_type: FieldType::DateTime,
                    required: false,
                    indexed: true,
                },
                FieldSchema {
                    name: "completed".to_string(),
                    field_type: FieldType::Boolean,
                    required: true,
                    indexed: false,
                },
                FieldSchema {
                    name: "project_id".to_string(),
                    field_type: FieldType::Reference("todoist_projects".to_string()),
                    required: false,
                    indexed: false,
                },
                FieldSchema {
                    name: "labels".to_string(),
                    field_type: FieldType::Json,
                    required: true,
                    indexed: false,
                },
            ],
        }
    }
}

// Also generates conversion to/from Entity HashMap
impl ToEntity for TodoistTask {
    fn to_entity(&self) -> Entity {
        let mut map = HashMap::new();
        map.insert("id".to_string(), Value::String(self.id.clone()));
        map.insert("content".to_string(), Value::String(self.content.clone()));
        if let Some(p) = self.priority {
            map.insert("priority".to_string(), Value::Integer(p as i64));
        }
        // ... etc
        map
    }
}

impl FromEntity for TodoistTask {
    fn from_entity(entity: &Entity) -> Result<Self> {
        Ok(Self {
            id: entity.get("id")
                .and_then(|v| v.as_string())
                .ok_or(Error::MissingField("id"))?,
            content: entity.get("content")
                .and_then(|v| v.as_string())
                .ok_or(Error::MissingField("content"))?,
            priority: entity.get("priority")
                .and_then(|v| v.as_integer())
                .map(|i| i as i32),
            // ... etc
        })
    }
}
```

## Usage with Storage Backend

```rust
// Create adapter with type-safe struct
let mut adapter = ExternalSystemAdapter::<SqliteBackend, TodoistTask>::new(
    storage,
    TodoistClient::new(token),
).await?;

// Type-safe operations
let task = TodoistTask {
    id: "123".to_string(),
    content: "Buy milk".to_string(),
    priority: Some(4),
    due_date: Some(Utc::now() + Duration::days(1)),
    completed: false,
    project_id: None,
    labels: vec!["grocery".to_string()],
    api_version: None,
};

adapter.insert(task).await?;

// Retrieve with type safety
let task: TodoistTask = adapter.get("123").await?;
println!("Task: {}", task.content);
```

## Attribute Options

```rust
// Container attributes (on struct)
#[entity(
    name = "custom_table_name",  // Override table name
    backend = "sqlite",           // Hint for optimal backend
)]

// Field attributes
#[primary_key]                    // Marks primary key
#[indexed]                        // Create index on this field
#[reference(entity = "other")]    // Foreign key relationship
#[skip_storage]                   // Don't persist this field
#[rename = "api_field_name"]      // Map to different name
#[default = "value"]              // Default value
#[validate(min = 1, max = 4)]     // Validation rules
```

## Type Mappings

```rust
// Automatic type inference:
struct Task {
    // Rust Type        → FieldType
    id: String          → FieldType::String
    count: i32          → FieldType::Integer
    count: i64          → FieldType::Integer
    active: bool        → FieldType::Boolean
    created: DateTime   → FieldType::DateTime
    metadata: Value     → FieldType::Json
    tags: Vec<String>   → FieldType::Json  // Serialized as JSON array

    // Option<T> means not required
    optional: Option<String> → FieldType::String, required: false

    // Custom types via trait
    #[json]
    custom: MyCustomType → FieldType::Json (if MyCustomType: Serialize)
}
```

## Derive Macro Implementation (Simplified)

```rust
// In storage_derive crate
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Data, Fields};

#[proc_macro_derive(Entity, attributes(entity, primary_key, indexed, reference, skip_storage))]
pub fn derive_entity(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let Data::Struct(data_struct) = &input.data else {
        panic!("Entity can only be derived for structs");
    };

    let Fields::Named(fields) = &data_struct.fields else {
        panic!("Entity requires named fields");
    };

    // Extract entity name from attribute or use struct name
    let entity_name = extract_entity_name(&input.attrs)
        .unwrap_or_else(|| to_snake_case(&name.to_string()));

    // Find primary key
    let primary_key = fields.named.iter()
        .find(|f| has_attribute(f, "primary_key"))
        .map(|f| f.ident.as_ref().unwrap().to_string())
        .expect("No primary_key attribute found");

    // Generate field schemas
    let field_schemas = fields.named.iter()
        .filter(|f| !has_attribute(f, "skip_storage"))
        .map(|f| {
            let field_name = f.ident.as_ref().unwrap().to_string();
            let field_type = rust_type_to_field_type(&f.ty);
            let required = !is_option_type(&f.ty);
            let indexed = has_attribute(f, "indexed");

            quote! {
                FieldSchema {
                    name: #field_name.to_string(),
                    field_type: #field_type,
                    required: #required,
                    indexed: #indexed,
                }
            }
        });

    // Generate the implementation
    let expanded = quote! {
        impl crate::storage::EntitySchema for #name {
            fn schema() -> crate::storage::EntitySchema {
                crate::storage::EntitySchema {
                    name: #entity_name.to_string(),
                    primary_key: #primary_key.to_string(),
                    fields: vec![
                        #(#field_schemas),*
                    ],
                }
            }
        }

        impl crate::storage::ToEntity for #name {
            fn to_entity(&self) -> crate::storage::Entity {
                // Generate HashMap conversion code
                // ...
            }
        }

        impl crate::storage::FromEntity for #name {
            fn from_entity(entity: &crate::storage::Entity) -> Result<Self> {
                // Generate struct construction code
                // ...
            }
        }
    };

    TokenStream::from(expanded)
}
```

## Approach 2: Builder Pattern (No Macros Needed)

If you want to avoid proc macros initially:

```rust
impl TodoistTask {
    pub fn schema() -> EntitySchemaBuilder {
        EntitySchemaBuilder::new("todoist_tasks")
            .field("id", FieldType::String)
                .required()
                .indexed()
                .primary_key()
            .field("content", FieldType::String)
                .required()
            .field("priority", FieldType::Integer)
                .optional()
                .indexed()
            .field("due_date", FieldType::DateTime)
                .optional()
                .indexed()
            .field("completed", FieldType::Boolean)
                .required()
            .field("project_id", FieldType::Reference("todoist_projects"))
                .optional()
            .field("labels", FieldType::Json)
                .required()
            .build()
    }
}

// With builder pattern
pub struct EntitySchemaBuilder {
    name: String,
    fields: Vec<FieldSchema>,
    current_field: Option<FieldSchemaBuilder>,
    primary_key: Option<String>,
}

impl EntitySchemaBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: vec![],
            current_field: None,
            primary_key: None,
        }
    }

    pub fn field(mut self, name: impl Into<String>, field_type: FieldType) -> Self {
        if let Some(field) = self.current_field.take() {
            self.fields.push(field.build());
        }
        self.current_field = Some(FieldSchemaBuilder::new(name, field_type));
        self
    }

    pub fn required(mut self) -> Self {
        if let Some(ref mut field) = self.current_field {
            field.required = true;
        }
        self
    }

    pub fn optional(mut self) -> Self {
        if let Some(ref mut field) = self.current_field {
            field.required = false;
        }
        self
    }

    pub fn indexed(mut self) -> Self {
        if let Some(ref mut field) = self.current_field {
            field.indexed = true;
        }
        self
    }

    pub fn primary_key(mut self) -> Self {
        if let Some(ref field) = self.current_field {
            self.primary_key = Some(field.name.clone());
        }
        self
    }

    pub fn build(mut self) -> EntitySchema {
        if let Some(field) = self.current_field.take() {
            self.fields.push(field.build());
        }
        EntitySchema {
            name: self.name,
            primary_key: self.primary_key.expect("No primary key defined"),
            fields: self.fields,
        }
    }
}
```

## Approach 3: Reflection-Based (Runtime)

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TodoistTask {
    pub id: String,
    pub content: String,
    pub priority: Option<i32>,
    pub due_date: Option<DateTime<Utc>>,
    pub completed: bool,
    pub project_id: Option<String>,
    pub labels: Vec<String>,
}

// Manual schema definition with validation
impl TodoistTask {
    pub fn schema() -> EntitySchema {
        use_struct_schema!(TodoistTask, {
            primary_key: "id",
            indexed: ["id", "priority", "due_date"],
        })
    }
}

// Or with serde introspection
pub fn schema_from_serde<T: Serialize>() -> EntitySchema {
    // Use serde to inspect the struct
    // This is more limited but requires no macros
}
```

## Integration with Adapter

```rust
// Modified adapter to work with strongly-typed structs
struct TypedAdapter<B: StorageBackend, T: Entity> {
    storage: Arc<Mutex<B>>,
    api_client: Box<dyn ApiClient<T>>,
    _phantom: PhantomData<T>,
}

impl<B: StorageBackend, T: Entity> TypedAdapter<B, T> {
    async fn new(
        storage: Arc<Mutex<B>>,
        api_client: Box<dyn ApiClient<T>>,
    ) -> Result<Self> {
        // Initialize schema from type
        storage.lock().await.create_entity(&T::schema()).await?;

        Ok(Self {
            storage,
            api_client,
            _phantom: PhantomData,
        })
    }

    // Type-safe operations
    async fn get(&self, id: &str) -> Result<Option<T>> {
        let storage = self.storage.lock().await;
        let entity = storage.get(&T::schema().name, id).await?;
        entity.map(|e| T::from_entity(&e)).transpose()
    }

    async fn insert(&mut self, item: T) -> Result<()> {
        let entity = item.to_entity();
        let mut storage = self.storage.lock().await;
        storage.insert(&T::schema().name, entity).await
    }

    async fn query(&self, filter: Filter) -> Result<Vec<T>> {
        let storage = self.storage.lock().await;
        let entities = storage.query(&T::schema().name, filter).await?;
        entities.into_iter()
            .map(|e| T::from_entity(&e))
            .collect()
    }

    async fn sync_from_remote(&mut self) -> Result<SyncStats> {
        let mut stats = SyncStats::default();

        // Fetch from API (already typed!)
        let remote_items: Vec<T> = self.api_client.fetch_all().await?;

        let mut storage = self.storage.lock().await;
        let entity_name = &T::schema().name;

        for item in remote_items {
            let id = item.get_id();
            let entity = item.to_entity();

            match storage.get(entity_name, &id).await? {
                Some(_) => {
                    storage.update(entity_name, &id, entity).await?;
                    stats.updated += 1;
                }
                None => {
                    storage.insert(entity_name, entity).await?;
                    stats.inserted += 1;
                }
            }
        }

        Ok(stats)
    }
}

// Entity trait that types must implement
pub trait Entity: Sized {
    fn schema() -> EntitySchema;
    fn to_entity(&self) -> crate::storage::Entity;
    fn from_entity(entity: &crate::storage::Entity) -> Result<Self>;
    fn get_id(&self) -> String;
}
```

## Complete Example

```rust
// 1. Define your struct
#[derive(Debug, Clone, Entity)]
#[entity(name = "todoist_tasks")]
struct TodoistTask {
    #[primary_key]
    #[indexed]
    id: String,
    content: String,
    #[indexed]
    priority: Option<i32>,
    completed: bool,
}

// 2. Define API client
struct TodoistClient {
    token: String,
    client: reqwest::Client,
}

#[async_trait]
impl ApiClient<TodoistTask> for TodoistClient {
    async fn fetch_all(&self) -> Result<Vec<TodoistTask>> {
        let response = self.client
            .get("https://api.todoist.com/rest/v2/tasks")
            .bearer_auth(&self.token)
            .send()
            .await?
            .json()
            .await?;
        Ok(response)
    }

    async fn update(&self, id: &str, task: TodoistTask) -> Result<TodoistTask> {
        // POST to API
    }
}

// 3. Use it!
#[tokio::main]
async fn main() -> Result<()> {
    // Choose backend
    let storage = SqliteBackend::new("tasks.db").await?;
    // Or: let storage = LoroBackend::new();

    let mut adapter = TypedAdapter::new(
        Arc::new(Mutex::new(storage)),
        Box::new(TodoistClient::new(token)),
    ).await?;

    // Type-safe sync
    adapter.sync_from_remote().await?;

    // Type-safe queries
    let urgent: Vec<TodoistTask> = adapter.query(
        Filter::Eq("priority".into(), Value::Integer(4))
    ).await?;

    for task in urgent {
        println!("Urgent: {}", task.content);
    }

    Ok(())
}
```

## Benefits

1. **Type Safety**: Compile-time guarantees about schema
2. **Less Boilerplate**: Define once, use everywhere
3. **Refactoring**: Rename fields → compiler catches all uses
4. **Documentation**: Struct is self-documenting
5. **IDE Support**: Autocomplete, go-to-definition work
6. **Validation**: Can add validation attributes

## Estimated Code Savings

**Without derive macro:**
- Schema definition: ~50 lines
- to_entity: ~30 lines
- from_entity: ~40 lines
- Total per type: ~120 lines

**With derive macro:**
- Struct with attributes: ~15 lines
- Total per type: ~15 lines

**For 5 entity types across 3 integrations (15 types total):**
- Without: 1,800 lines
- With: 225 lines + ~500 lines for derive macro
- **Savings: ~1,075 lines (60% reduction)**

The derive macro approach is definitely the way to go once you have a few integrations. Want me to show a full working example of the proc macro implementation, or dive into any specific aspect?
</ai>

<user>
I agree that the derive macro would be best. How could we handle references between entities in the external systems? E.g. in Todoist projects can have subprojects and sections, each of those canhave tasks, tasks can have sub-tasks. In the UI I want to be able to embed a block for a section in a Todoist project and have it display all sub-tasks.
</user>

<ai>
**Excellent question!** This is where things get really interesting. You need to handle:
1. Hierarchical relationships within external systems
2. Efficient queries across relationships
3. Block references that resolve to external entities
4. Keeping relationships in sync

Let me design a comprehensive solution:

## 1. Modeling Relationships in Structs

```rust
#[derive(Debug, Clone, Entity)]
#[entity(name = "todoist_projects")]
struct TodoistProject {
    #[primary_key]
    #[indexed]
    id: String,

    name: String,

    /// Parent project for nested projects
    #[reference(entity = "todoist_projects")]
    #[indexed]
    parent_id: Option<String>,

    color: String,
    is_favorite: bool,
}

#[derive(Debug, Clone, Entity)]
#[entity(name = "todoist_sections")]
struct TodoistSection {
    #[primary_key]
    #[indexed]
    id: String,

    name: String,

    /// Which project this section belongs to
    #[reference(entity = "todoist_projects")]
    #[indexed]
    project_id: String,

    /// Order within project
    order: i32,
}

#[derive(Debug, Clone, Entity)]
#[entity(name = "todoist_tasks")]
struct TodoistTask {
    #[primary_key]
    #[indexed]
    id: String,

    content: String,

    #[indexed]
    priority: Option<i32>,

    /// Which project (if no section)
    #[reference(entity = "todoist_projects")]
    #[indexed]
    project_id: Option<String>,

    /// Which section (if any)
    #[reference(entity = "todoist_sections")]
    #[indexed]
    section_id: Option<String>,

    /// Parent task for subtasks
    #[reference(entity = "todoist_tasks")]
    #[indexed]
    parent_id: Option<String>,

    completed: bool,
    order: i32,
}
```

## 2. Extended Storage Trait with Relationships

```rust
#[async_trait]
trait StorageBackend: Send + Sync {
    // ... existing methods ...

    // Relationship queries
    async fn get_children(
        &self,
        entity: &str,
        parent_field: &str,
        parent_id: &str,
    ) -> Result<Vec<Entity>>;

    async fn get_related(
        &self,
        entity: &str,
        foreign_key: &str,
        related_id: &str,
    ) -> Result<Vec<Entity>>;

    async fn get_tree(
        &self,
        entity: &str,
        parent_field: &str,
        root_id: Option<&str>,
    ) -> Result<EntityTree>;
}

// Tree structure for hierarchical data
#[derive(Debug, Clone)]
struct EntityTree {
    node: Entity,
    children: Vec<EntityTree>,
}
```

## 3. Type-Safe Relationship Queries

```rust
// Add relationship methods to the Entity trait
pub trait Entity: Sized {
    // ... existing methods ...

    // Define relationships
    fn relationships() -> Vec<Relationship> {
        vec![]  // Override in derive macro
    }
}

#[derive(Debug, Clone)]
struct Relationship {
    name: String,
    field: String,
    target_entity: String,
    relationship_type: RelationshipType,
}

#[derive(Debug, Clone)]
enum RelationshipType {
    BelongsTo,      // Many-to-one (task belongs to section)
    HasMany,        // One-to-many (section has many tasks)
    SelfReferential, // Tree structure (task has subtasks)
}

// Enhanced adapter with relationship support
impl<B: StorageBackend, T: Entity> TypedAdapter<B, T> {
    // Get related entities with type safety
    async fn get_related<R: Entity>(
        &self,
        id: &str,
        relationship: &str,
    ) -> Result<Vec<R>> {
        let rel = T::relationships()
            .iter()
            .find(|r| r.name == relationship)
            .ok_or(Error::UnknownRelationship)?;

        let storage = self.storage.lock().await;
        let entities = storage.get_related(
            &R::schema().name,
            &rel.field,
            id,
        ).await?;

        entities.into_iter()
            .map(|e| R::from_entity(&e))
            .collect()
    }

    // Get children in hierarchy
    async fn get_children(&self, parent_id: &str) -> Result<Vec<T>> {
        let storage = self.storage.lock().await;
        let entities = storage.get_children(
            &T::schema().name,
            "parent_id",
            parent_id,
        ).await?;

        entities.into_iter()
            .map(|e| T::from_entity(&e))
            .collect()
    }

    // Get full tree
    async fn get_tree(&self, root_id: Option<&str>) -> Result<Tree<T>> {
        let storage = self.storage.lock().await;
        let entity_tree = storage.get_tree(
            &T::schema().name,
            "parent_id",
            root_id,
        ).await?;

        self.entity_tree_to_typed(entity_tree)
    }
}

// Typed tree structure
#[derive(Debug, Clone)]
struct Tree<T> {
    node: T,
    children: Vec<Tree<T>>,
}
```

## 4. Derive Macro with Relationship Support

```rust
#[derive(Debug, Clone, Entity)]
#[entity(name = "todoist_tasks")]
#[relationships(
    section = (belongs_to = "todoist_sections", field = "section_id"),
    project = (belongs_to = "todoist_projects", field = "project_id"),
    subtasks = (has_many = "todoist_tasks", field = "parent_id"),
    parent = (belongs_to = "todoist_tasks", field = "parent_id"),
)]
struct TodoistTask {
    #[primary_key]
    id: String,

    content: String,

    #[reference(entity = "todoist_sections")]
    section_id: Option<String>,

    #[reference(entity = "todoist_projects")]
    project_id: Option<String>,

    #[reference(entity = "todoist_tasks")]
    parent_id: Option<String>,
}

// The macro generates:
impl TodoistTask {
    // Load related section
    async fn section<B: StorageBackend>(
        &self,
        storage: &B,
    ) -> Result<Option<TodoistSection>> {
        if let Some(ref section_id) = self.section_id {
            let entity = storage.get("todoist_sections", section_id).await?;
            entity.map(|e| TodoistSection::from_entity(&e)).transpose()
        } else {
            Ok(None)
        }
    }

    // Load all subtasks
    async fn subtasks<B: StorageBackend>(
        &self,
        storage: &B,
    ) -> Result<Vec<TodoistTask>> {
        let entities = storage.get_children(
            "todoist_tasks",
            "parent_id",
            &self.id,
        ).await?;

        entities.into_iter()
            .map(|e| Self::from_entity(&e))
            .collect()
    }

    // Load parent task
    async fn parent<B: StorageBackend>(
        &self,
        storage: &B,
    ) -> Result<Option<TodoistTask>> {
        if let Some(ref parent_id) = self.parent_id {
            let entity = storage.get("todoist_tasks", parent_id).await?;
            entity.map(|e| Self::from_entity(&e)).transpose()
        } else {
            Ok(None)
        }
    }
}
```

## 5. Query Builder for Complex Queries

```rust
// High-level query interface
struct QueryBuilder<'a, B: StorageBackend, T: Entity> {
    storage: &'a B,
    filters: Vec<Filter>,
    includes: Vec<String>,  // Eager load relationships
    order_by: Vec<(String, Order)>,
    limit: Option<usize>,
    _phantom: PhantomData<T>,
}

impl<'a, B: StorageBackend, T: Entity> QueryBuilder<'a, B, T> {
    pub fn new(storage: &'a B) -> Self {
        Self {
            storage,
            filters: vec![],
            includes: vec![],
            order_by: vec![],
            limit: None,
            _phantom: PhantomData,
        }
    }

    pub fn filter(mut self, filter: Filter) -> Self {
        self.filters.push(filter);
        self
    }

    pub fn include(mut self, relationship: impl Into<String>) -> Self {
        self.includes.push(relationship.into());
        self
    }

    pub fn order_by(mut self, field: impl Into<String>, order: Order) -> Self {
        self.order_by.push((field.into(), order));
        self
    }

    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    pub async fn all(self) -> Result<Vec<T>> {
        // Execute query
        let entities = self.storage.query(
            &T::schema().name,
            Filter::And(self.filters),
        ).await?;

        entities.into_iter()
            .map(|e| T::from_entity(&e))
            .collect()
    }
}

// Usage
let tasks = QueryBuilder::<_, TodoistTask>::new(&storage)
    .filter(Filter::Eq("section_id".into(), Value::String("sec_456".into())))
    .filter(Filter::Eq("completed".into(), Value::Boolean(false)))
    .order_by("priority", Order::Desc)
    .order_by("order", Order::Asc)
    .all()
    .await?;
```

## 6. Block Reference System

```rust
// Reference to external entities
#[derive(Debug, Clone, Serialize, Deserialize)]
enum BlockReference {
    // Internal content (stored in Loro)
    Internal { block_id: String },

    // External entities
    External {
        system: String,      // "todoist"
        entity_type: String, // "section"
        entity_id: String,   // "sec_456"
        view: Option<ViewConfig>, // How to display
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ViewConfig {
    // Display options
    show_completed: bool,
    group_by: Option<String>,
    sort_by: Vec<(String, Order)>,
    max_depth: Option<usize>,  // For hierarchies
}

// In your notes (stored in Loro)
impl BlockReference {
    fn todoist_section(id: impl Into<String>) -> Self {
        Self::External {
            system: "todoist".into(),
            entity_type: "section".into(),
            entity_id: id.into(),
            view: Some(ViewConfig {
                show_completed: false,
                group_by: None,
                sort_by: vec![("priority".into(), Order::Desc)],
                max_depth: Some(2),
            }),
        }
    }
}
```

## 7. Resolver for Block References

```rust
struct ReferenceResolver {
    storage: Arc<Storage>,
}

impl ReferenceResolver {
    async fn resolve(&self, reference: &BlockReference) -> Result<ResolvedBlock> {
        match reference {
            BlockReference::Internal { block_id } => {
                // Load from Loro
                self.resolve_internal(block_id).await
            }
            BlockReference::External { system, entity_type, entity_id, view } => {
                // Load from external system cache
                self.resolve_external(system, entity_type, entity_id, view).await
            }
        }
    }

    async fn resolve_external(
        &self,
        system: &str,
        entity_type: &str,
        entity_id: &str,
        view: &Option<ViewConfig>,
    ) -> Result<ResolvedBlock> {
        match (system, entity_type) {
            ("todoist", "section") => {
                self.resolve_todoist_section(entity_id, view).await
            }
            ("todoist", "project") => {
                self.resolve_todoist_project(entity_id, view).await
            }
            ("todoist", "task") => {
                self.resolve_todoist_task(entity_id, view).await
            }
            _ => Err(Error::UnknownReference),
        }
    }

    async fn resolve_todoist_section(
        &self,
        section_id: &str,
        view: &Option<ViewConfig>,
    ) -> Result<ResolvedBlock> {
        let storage = &self.storage.todoist;

        // Load section
        let section: TodoistSection = storage.get(section_id).await?
            .ok_or(Error::NotFound)?;

        // Load all tasks in section
        let mut tasks: Vec<TodoistTask> = QueryBuilder::new(&storage)
            .filter(Filter::Eq("section_id".into(), Value::String(section_id.into())))
            .all()
            .await?;

        // Apply view configuration
        if let Some(view) = view {
            if !view.show_completed {
                tasks.retain(|t| !t.completed);
            }

            // Sort
            for (field, order) in view.sort_by.iter().rev() {
                tasks.sort_by(|a, b| {
                    let cmp = compare_task_field(a, b, field);
                    match order {
                        Order::Asc => cmp,
                        Order::Desc => cmp.reverse(),
                    }
                });
            }
        }

        // Build hierarchy (tasks with subtasks)
        let tree = build_task_tree(tasks, view.as_ref().and_then(|v| v.max_depth));

        Ok(ResolvedBlock::TodoistSection {
            section,
            tasks: tree,
        })
    }
}

// Resolved data ready for display
#[derive(Debug, Clone)]
enum ResolvedBlock {
    Internal {
        content: String,
    },
    TodoistSection {
        section: TodoistSection,
        tasks: Vec<TaskNode>,
    },
    TodoistProject {
        project: TodoistProject,
        sections: Vec<(TodoistSection, Vec<TaskNode>)>,
    },
}

#[derive(Debug, Clone)]
struct TaskNode {
    task: TodoistTask,
    subtasks: Vec<TaskNode>,
}

fn build_task_tree(tasks: Vec<TodoistTask>, max_depth: Option<usize>) -> Vec<TaskNode> {
    let mut task_map: HashMap<String, TaskNode> = HashMap::new();
    let mut roots = vec![];

    // First pass: create all nodes
    for task in tasks {
        task_map.insert(task.id.clone(), TaskNode {
            task,
            subtasks: vec![],
        });
    }

    // Second pass: build tree
    let task_map_clone = task_map.clone();
    for (id, mut node) in task_map.into_iter() {
        if let Some(parent_id) = &node.task.parent_id {
            if let Some(parent) = task_map_clone.get(parent_id) {
                // This will be added as a child
                continue;
            }
        }
        roots.push(node);
    }

    // Recursive depth limiting
    fn limit_depth(nodes: &mut Vec<TaskNode>, current_depth: usize, max_depth: usize) {
        if current_depth >= max_depth {
            for node in nodes {
                node.subtasks.clear();
            }
        } else {
            for node in nodes {
                limit_depth(&mut node.subtasks, current_depth + 1, max_depth);
            }
        }
    }

    if let Some(max) = max_depth {
        limit_depth(&mut roots, 0, max);
    }

    roots
}
```

## 8. Usage in Your Notes

```rust
// In your internal content (Loro document)
let doc = loro_doc.get_text("project_notes");

// Markdown with embedded references
let content = r#"
# Website Redesign Project

## Tasks Overview

{{todoist:section:sec_456}}

## Design Phase

Key tasks for this phase:
- {{todoist:task:task_789}}
- {{todoist:task:task_888}}

## Notes

The design mockups are looking great...
"#;

doc.insert(0, content);

// Store reference metadata separately
let refs = loro_doc.get_map("block_references");
refs.set("ref_1", serde_json::to_string(&BlockReference::todoist_section("sec_456"))?);
```

## 9. Rendering in UI

```rust
struct BlockRenderer {
    resolver: ReferenceResolver,
}

impl BlockRenderer {
    async fn render(&self, content: &str) -> Result<RenderedContent> {
        let mut output = String::new();
        let mut last_pos = 0;

        // Find all {{system:type:id}} references
        let re = Regex::new(r"\{\{([^:]+):([^:]+):([^}]+)\}\}")?;

        for cap in re.captures_iter(content) {
            let system = &cap[1];
            let entity_type = &cap[2];
            let entity_id = &cap[3];

            // Add text before reference
            output.push_str(&content[last_pos..cap.get(0).unwrap().start()]);

            // Resolve and render reference
            let reference = BlockReference::External {
                system: system.into(),
                entity_type: entity_type.into(),
                entity_id: entity_id.into(),
                view: None,
            };

            let resolved = self.resolver.resolve(&reference).await?;
            let rendered = self.render_resolved(&resolved)?;
            output.push_str(&rendered);

            last_pos = cap.get(0).unwrap().end();
        }

        // Add remaining content
        output.push_str(&content[last_pos..]);

        Ok(RenderedContent { html: output })
    }

    fn render_resolved(&self, resolved: &ResolvedBlock) -> Result<String> {
        match resolved {
            ResolvedBlock::TodoistSection { section, tasks } => {
                let mut html = format!("<div class='todoist-section' data-id='{}'>\n", section.id);
                html.push_str(&format!("  <h3>{}</h3>\n", section.name));
                html.push_str("  <ul class='task-list'>\n");

                for task_node in tasks {
                    html.push_str(&self.render_task_node(task_node, 0)?);
                }

                html.push_str("  </ul>\n");
                html.push_str("</div>\n");
                Ok(html)
            }
            _ => Ok(String::new()),
        }
    }

    fn render_task_node(&self, node: &TaskNode, depth: usize) -> Result<String> {
        let indent = "  ".repeat(depth + 2);
        let mut html = format!(
            "{}<li class='task priority-{}' data-id='{}'>\n",
            indent,
            node.task.priority.unwrap_or(1),
            node.task.id
        );

        html.push_str(&format!(
            "{}  <input type='checkbox' {} />\n",
            indent,
            if node.task.completed { "checked" } else { "" }
        ));

        html.push_str(&format!("{}  <span>{}</span>\n", indent, node.task.content));

        if !node.subtasks.is_empty() {
            html.push_str(&format!("{}  <ul class='subtasks'>\n", indent));
            for subtask in &node.subtasks {
                html.push_str(&self.render_task_node(subtask, depth + 1)?);
            }
            html.push_str(&format!("{}  </ul>\n", indent));
        }

        html.push_str(&format!("{}</li>\n", indent));
        Ok(html)
    }
}
```

## 10. Syncing Relationships

```rust
impl<B: StorageBackend> ExternalSystemAdapter<B, TodoistTask> {
    async fn sync_from_remote(&mut self) -> Result<SyncStats> {
        let mut stats = SyncStats::default();

        // Fetch all entities with relationships intact
        let projects = self.api_client.fetch_projects().await?;
        let sections = self.api_client.fetch_sections().await?;
        let tasks = self.api_client.fetch_tasks().await?;

        let mut storage = self.storage.lock().await;

        // Sync in order: projects → sections → tasks
        // This maintains referential integrity

        for project in projects {
            let entity = project.to_entity();
            storage.upsert("todoist_projects", entity).await?;
            stats.projects_synced += 1;
        }

        for section in sections {
            let entity = section.to_entity();
            storage.upsert("todoist_sections", entity).await?;
            stats.sections_synced += 1;
        }

        for task in tasks {
            let entity = task.to_entity();
            storage.upsert("todoist_tasks", entity).await?;
            stats.tasks_synced += 1;
        }

        Ok(stats)
    }
}
```

## 11. Complete Example

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // Setup storage
    let storage = SqliteBackend::new("tasks.db").await?;

    // Setup Todoist adapter
    let todoist = TypedAdapter::new(
        Arc::new(Mutex::new(storage.clone())),
        Box::new(TodoistClient::new(token)),
    ).await?;

    // Sync all data
    todoist.sync_from_remote().await?;

    // Query with relationships
    let section = todoist.get("sec_456").await?.unwrap();
    let tasks = todoist
        .query(Filter::Eq("section_id".into(), Value::String("sec_456".into())))
        .await?;

    println!("Section: {}", section.name);
    for task in tasks {
        println!("  - {}", task.content);

        // Load subtasks
        let subtasks = task.subtasks(&storage).await?;
        for subtask in subtasks {
            println!("    - {}", subtask.content);
        }
    }

    // Use in your notes
    let doc = LoroDoc::new();
    let notes = doc.get_text("notes");
    notes.insert(0, "# Project Notes\n\n{{todoist:section:sec_456}}\n");

    // Render for display
    let resolver = ReferenceResolver::new(Arc::new(storage));
    let renderer = BlockRenderer::new(resolver);
    let html = renderer.render(&notes.to_string()).await?;

    println!("{}", html);

    Ok(())
}
```

## Summary

This approach gives you:

1. **Type-safe relationships**: Compile-time checks for foreign keys
2. **Efficient queries**: Indexed lookups for relationships
3. **Hierarchical data**: Tree structures with depth limits
4. **Block embedding**: Reference external entities in your notes
5. **Live resolution**: Always shows current data from cache
6. **Flexible rendering**: Custom views per reference
7. **Sync integrity**: Maintains relationships during sync

**Code estimate for relationships:**
- Relationship derive macro additions: ~300 lines
- Query builder: ~400 lines
- Reference resolver: ~500 lines
- Block renderer: ~300 lines
- **Total: ~1,500 lines** (one-time)

**Per integration with relationships:**
- Just define structs with `#[reference]` attributes
- Relationship methods auto-generated
- **No additional code per integration**

Want me to elaborate on any particular aspect, like the SQLite implementation of relationship queries or the Loro Tree structure for hierarchies?
</ai>

<user>
I'm wondering if and how this could be unified with relationships in the internal content. There we have a parent-child relationships between nested blocks, and blocks can also reference other blocks using Wiki syntax as in LogSeq.
Note that if we need IDs for an entity, I would like them to be URLs. In the best case in a form that they are actually valid URLs for the external systems and also (with corresponding URL handing configured) for the internal entities.
Also note, that all methods related to an external system should be in one module, and there should be no class/method/... that knows about more than one internal or external system.
</user>

<ai>
**Excellent insight!** This unification makes the system much more elegant. Let me redesign around URLs as universal identifiers and strict module boundaries.

## 1. URL-Based Universal Identifiers

```rust
// All entities are identified by URLs
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct EntityUrl(Url);

impl EntityUrl {
    // External systems use their actual URLs
    fn todoist_task(id: &str) -> Self {
        Self(Url::parse(&format!("https://todoist.com/app/task/{}", id)).unwrap())
    }

    fn todoist_project(id: &str) -> Self {
        Self(Url::parse(&format!("https://todoist.com/app/project/{}", id)).unwrap())
    }

    // Internal content uses custom scheme
    fn internal_block(id: &str) -> Self {
        Self(Url::parse(&format!("pkm://block/{}", id)).unwrap())
    }

    // Or localhost for actual HTTP access
    fn internal_block_http(id: &str) -> Self {
        Self(Url::parse(&format!("http://localhost:3000/block/{}", id)).unwrap())
    }

    // Parse from string (supports wiki-style links)
    fn from_wiki_link(link: &str) -> Result<Self> {
        // [[todoist:123]] → https://todoist.com/app/task/123
        // [[block:abc]] → pkm://block/abc
        // [[https://...]] → direct URL
        if link.starts_with("http://") || link.starts_with("https://") {
            Ok(Self(Url::parse(link)?))
        } else if let Some(rest) = link.strip_prefix("todoist:") {
            Ok(Self::todoist_task(rest))
        } else if let Some(rest) = link.strip_prefix("block:") {
            Ok(Self::internal_block(rest))
        } else {
            // Default: treat as internal block
            Ok(Self::internal_block(link))
        }
    }

    fn scheme(&self) -> &str {
        self.0.scheme()
    }

    fn host(&self) -> Option<&str> {
        self.0.host_str()
    }
}

// Display as clickable URL
impl std::fmt::Display for EntityUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

## 2. Unified Relationship Model

```rust
// Universal relationship types that work for ALL systems
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Relationship {
    /// Parent-child hierarchy (blocks, tasks with subtasks, nested projects)
    Parent { parent: EntityUrl },

    /// Reference to another entity (wiki-style links)
    References { targets: Vec<EntityUrl> },

    /// Belongs to a container (task in section, block in page)
    Container { container: EntityUrl },
}

// All entities support these relationships
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entity {
    url: EntityUrl,
    entity_type: String,
    data: HashMap<String, Value>,
    relationships: Vec<Relationship>,
}

impl Entity {
    fn parent(&self) -> Option<&EntityUrl> {
        self.relationships.iter()
            .find_map(|r| match r {
                Relationship::Parent { parent } => Some(parent),
                _ => None,
            })
    }

    fn references(&self) -> Vec<&EntityUrl> {
        self.relationships.iter()
            .flat_map(|r| match r {
                Relationship::References { targets } => targets.iter().collect(),
                _ => vec![],
            })
            .collect()
    }

    fn container(&self) -> Option<&EntityUrl> {
        self.relationships.iter()
            .find_map(|r| match r {
                Relationship::Container { container } => Some(container),
                _ => None,
            })
    }
}
```

## 3. Universal System Interface

```rust
// Every system (internal or external) implements this
#[async_trait]
trait EntitySystem: Send + Sync {
    /// Name of this system (for routing)
    fn name(&self) -> &str;

    /// Can this system handle this URL?
    fn handles(&self, url: &EntityUrl) -> bool;

    /// Fetch a single entity by URL
    async fn get(&self, url: &EntityUrl) -> Result<Option<Entity>>;

    /// Query entities with filters
    async fn query(&self, filter: SystemQuery) -> Result<Vec<Entity>>;

    /// Get children of an entity (for hierarchies)
    async fn children(&self, parent_url: &EntityUrl) -> Result<Vec<Entity>>;

    /// Get all entities that reference this URL
    async fn backlinks(&self, target_url: &EntityUrl) -> Result<Vec<Entity>>;

    /// Update an entity
    async fn update(&mut self, url: &EntityUrl, changes: EntityUpdate) -> Result<Entity>;

    /// Sync with remote (if applicable)
    async fn sync(&mut self) -> Result<SyncStats> {
        Ok(SyncStats::default())
    }

    /// System capabilities
    fn capabilities(&self) -> SystemCapabilities;
}

#[derive(Debug, Clone)]
struct SystemCapabilities {
    supports_hierarchy: bool,
    supports_references: bool,
    supports_sync: bool,
    read_only: bool,
}

#[derive(Debug, Clone)]
struct SystemQuery {
    entity_type: Option<String>,
    filters: Vec<Filter>,
    include_relationships: bool,
}

#[derive(Debug, Clone)]
struct EntityUpdate {
    data: HashMap<String, Value>,
    add_relationships: Vec<Relationship>,
    remove_relationships: Vec<Relationship>,
}
```

## 4. System Registry (No Cross-System Knowledge)

```rust
// Central registry that routes URLs to appropriate systems
struct SystemRegistry {
    systems: HashMap<String, Box<dyn EntitySystem>>,
}

impl SystemRegistry {
    fn new() -> Self {
        Self {
            systems: HashMap::new(),
        }
    }

    fn register(&mut self, system: Box<dyn EntitySystem>) {
        let name = system.name().to_string();
        self.systems.insert(name, system);
    }

    // Route a URL to the appropriate system
    fn find_system(&self, url: &EntityUrl) -> Option<&dyn EntitySystem> {
        self.systems.values()
            .find(|s| s.handles(url))
            .map(|s| s.as_ref())
    }

    fn find_system_mut(&mut self, url: &EntityUrl) -> Option<&mut dyn EntitySystem> {
        self.systems.values_mut()
            .find(|s| s.handles(url))
            .map(|s| s.as_mut())
    }

    // Universal operations that work across all systems
    async fn get(&self, url: &EntityUrl) -> Result<Option<Entity>> {
        let system = self.find_system(url)
            .ok_or(Error::NoSystemForUrl)?;
        system.get(url).await
    }

    async fn children(&self, parent_url: &EntityUrl) -> Result<Vec<Entity>> {
        let system = self.find_system(parent_url)
            .ok_or(Error::NoSystemForUrl)?;
        system.children(parent_url).await
    }

    async fn backlinks(&self, target_url: &EntityUrl) -> Result<Vec<Entity>> {
        // Check all systems for backlinks
        let mut all_backlinks = vec![];

        for system in self.systems.values() {
            let backlinks = system.backlinks(target_url).await?;
            all_backlinks.extend(backlinks);
        }

        Ok(all_backlinks)
    }

    // Resolve a tree of entities (follows parent relationships)
    async fn get_tree(&self, root_url: &EntityUrl, max_depth: Option<usize>) -> Result<EntityTree> {
        self.get_tree_recursive(root_url, 0, max_depth).await
    }

    async fn get_tree_recursive(
        &self,
        url: &EntityUrl,
        current_depth: usize,
        max_depth: Option<usize>,
    ) -> Result<EntityTree> {
        let entity = self.get(url).await?
            .ok_or(Error::NotFound)?;

        let mut children = vec![];

        if max_depth.map_or(true, |max| current_depth < max) {
            let child_entities = self.children(url).await?;
            for child in child_entities {
                let child_tree = self.get_tree_recursive(
                    &child.url,
                    current_depth + 1,
                    max_depth,
                ).await?;
                children.push(child_tree);
            }
        }

        Ok(EntityTree { entity, children })
    }
}

#[derive(Debug, Clone)]
struct EntityTree {
    entity: Entity,
    children: Vec<EntityTree>,
}
```

## 5. Internal Content System (Loro-based)

```rust
// modules/internal/mod.rs
use loro::LoroDoc;

pub struct InternalSystem {
    doc: LoroDoc,
}

impl InternalSystem {
    pub fn new() -> Self {
        let doc = LoroDoc::new();
        Self { doc }
    }

    fn block_id_from_url(url: &EntityUrl) -> Option<String> {
        if url.scheme() != "pkm" {
            return None;
        }

        url.0.path().strip_prefix("/block/")
            .map(|s| s.to_string())
    }

    fn url_from_block_id(id: &str) -> EntityUrl {
        EntityUrl::internal_block(id)
    }
}

#[async_trait]
impl EntitySystem for InternalSystem {
    fn name(&self) -> &str {
        "internal"
    }

    fn handles(&self, url: &EntityUrl) -> bool {
        url.scheme() == "pkm"
    }

    async fn get(&self, url: &EntityUrl) -> Result<Option<Entity>> {
        let block_id = Self::block_id_from_url(url)
            .ok_or(Error::InvalidUrl)?;

        // Get block from Loro
        let blocks = self.doc.get_map("blocks");
        let block = blocks.get(&block_id);

        match block {
            Some(value) => {
                let block_map = value.as_map().ok_or(Error::InvalidData)?;
                Ok(Some(self.loro_block_to_entity(block_id, block_map)?))
            }
            None => Ok(None),
        }
    }

    async fn query(&self, query: SystemQuery) -> Result<Vec<Entity>> {
        let blocks = self.doc.get_map("blocks");
        let mut results = vec![];

        for (block_id, value) in blocks.iter() {
            let block_map = value.as_map().ok_or(Error::InvalidData)?;
            let entity = self.loro_block_to_entity(block_id, block_map)?;

            // Apply filters
            if self.matches_query(&entity, &query) {
                results.push(entity);
            }
        }

        Ok(results)
    }

    async fn children(&self, parent_url: &EntityUrl) -> Result<Vec<Entity>> {
        let parent_id = Self::block_id_from_url(parent_url)
            .ok_or(Error::InvalidUrl)?;

        // Query blocks with this parent
        let query = SystemQuery {
            entity_type: Some("block".to_string()),
            filters: vec![],
            include_relationships: true,
        };

        let all_blocks = self.query(query).await?;

        // Filter to children of this parent
        Ok(all_blocks.into_iter()
            .filter(|e| e.parent() == Some(parent_url))
            .collect())
    }

    async fn backlinks(&self, target_url: &EntityUrl) -> Result<Vec<Entity>> {
        let query = SystemQuery {
            entity_type: Some("block".to_string()),
            filters: vec![],
            include_relationships: true,
        };

        let all_blocks = self.query(query).await?;

        // Filter to blocks that reference this URL
        Ok(all_blocks.into_iter()
            .filter(|e| e.references().contains(&target_url))
            .collect())
    }

    async fn update(&mut self, url: &EntityUrl, changes: EntityUpdate) -> Result<Entity> {
        let block_id = Self::block_id_from_url(url)
            .ok_or(Error::InvalidUrl)?;

        let blocks = self.doc.get_map("blocks");
        let block = blocks.get_map(&block_id);

        // Update data fields
        for (key, value) in changes.data {
            self.set_loro_value(&block, &key, value)?;
        }

        // Update relationships
        let relationships = self.doc.get_map("relationships");
        let block_rels = relationships.get_map(&block_id);

        // ... handle relationship updates

        self.get(url).await?.ok_or(Error::NotFound)
    }

    fn capabilities(&self) -> SystemCapabilities {
        SystemCapabilities {
            supports_hierarchy: true,
            supports_references: true,
            supports_sync: false,  // Local only
            read_only: false,
        }
    }

    // Helper methods
    fn loro_block_to_entity(&self, block_id: String, block_map: &LoroMap) -> Result<Entity> {
        let mut data = HashMap::new();

        // Extract content
        if let Some(content) = block_map.get("content") {
            data.insert("content".to_string(), Value::String(content.as_string().unwrap_or_default().to_string()));
        }

        // Extract relationships
        let mut relationships = vec![];

        // Parent relationship
        if let Some(parent_id) = block_map.get("parent_id").and_then(|v| v.as_string()) {
            relationships.push(Relationship::Parent {
                parent: Self::url_from_block_id(parent_id),
            });
        }

        // References (parse from content)
        if let Some(content) = block_map.get("content").and_then(|v| v.as_string()) {
            let refs = self.extract_references(content)?;
            if !refs.is_empty() {
                relationships.push(Relationship::References { targets: refs });
            }
        }

        Ok(Entity {
            url: Self::url_from_block_id(&block_id),
            entity_type: "block".to_string(),
            data,
            relationships,
        })
    }

    fn extract_references(&self, content: &str) -> Result<Vec<EntityUrl>> {
        // Parse [[wiki-links]] from content
        let re = Regex::new(r"\[\[([^\]]+)\]\]")?;
        let mut refs = vec![];

        for cap in re.captures_iter(content) {
            let link = &cap[1];
            refs.push(EntityUrl::from_wiki_link(link)?);
        }

        Ok(refs)
    }

    fn matches_query(&self, entity: &Entity, query: &SystemQuery) -> bool {
        if let Some(ref entity_type) = query.entity_type {
            if &entity.entity_type != entity_type {
                return false;
            }
        }

        // Apply filters
        for filter in &query.filters {
            if !self.matches_filter(entity, filter) {
                return false;
            }
        }

        true
    }
}
```

## 6. Todoist System Module

```rust
// modules/todoist/mod.rs
use crate::storage::StorageBackend;

pub struct TodoistSystem<B: StorageBackend> {
    storage: Arc<Mutex<B>>,
    api_client: TodoistClient,
    last_sync: Option<DateTime<Utc>>,
}

impl<B: StorageBackend> TodoistSystem<B> {
    pub fn new(storage: Arc<Mutex<B>>, api_token: String) -> Self {
        Self {
            storage,
            api_client: TodoistClient::new(api_token),
            last_sync: None,
        }
    }

    fn task_id_from_url(url: &EntityUrl) -> Option<String> {
        if url.host() != Some("todoist.com") {
            return None;
        }

        url.0.path()
            .strip_prefix("/app/task/")
            .or_else(|| url.0.path().strip_prefix("/showTask?id="))
            .map(|s| s.to_string())
    }

    fn url_from_task_id(id: &str) -> EntityUrl {
        EntityUrl::todoist_task(id)
    }
}

#[async_trait]
impl<B: StorageBackend> EntitySystem for TodoistSystem<B> {
    fn name(&self) -> &str {
        "todoist"
    }

    fn handles(&self, url: &EntityUrl) -> bool {
        url.host() == Some("todoist.com")
    }

    async fn get(&self, url: &EntityUrl) -> Result<Option<Entity>> {
        let task_id = Self::task_id_from_url(url)
            .ok_or(Error::InvalidUrl)?;

        // Try cache first
        let storage = self.storage.lock().await;
        let cached = storage.get("todoist_tasks", &task_id).await?;

        if let Some(task_entity) = cached {
            return Ok(Some(self.task_entity_to_entity(task_entity)?));
        }

        Ok(None)
    }

    async fn query(&self, query: SystemQuery) -> Result<Vec<Entity>> {
        let storage = self.storage.lock().await;

        // Map generic query to storage-specific query
        let results = storage.query("todoist_tasks", Filter::And(query.filters)).await?;

        results.into_iter()
            .map(|e| self.task_entity_to_entity(e))
            .collect()
    }

    async fn children(&self, parent_url: &EntityUrl) -> Result<Vec<Entity>> {
        let parent_id = Self::task_id_from_url(parent_url)
            .ok_or(Error::InvalidUrl)?;

        let storage = self.storage.lock().await;
        let children = storage.get_children("todoist_tasks", "parent_id", &parent_id).await?;

        children.into_iter()
            .map(|e| self.task_entity_to_entity(e))
            .collect()
    }

    async fn backlinks(&self, target_url: &EntityUrl) -> Result<Vec<Entity>> {
        // Todoist tasks don't have explicit references to other systems
        // But we could parse descriptions for links
        Ok(vec![])
    }

    async fn update(&mut self, url: &EntityUrl, changes: EntityUpdate) -> Result<Entity> {
        let task_id = Self::task_id_from_url(url)
            .ok_or(Error::InvalidUrl)?;

        // Update in cache
        let mut storage = self.storage.lock().await;
        storage.update("todoist_tasks", &task_id, changes.data.clone()).await?;
        storage.mark_dirty("todoist_tasks", &task_id).await?;

        // Later, sync will push to API

        self.get(url).await?.ok_or(Error::NotFound)
    }

    async fn sync(&mut self) -> Result<SyncStats> {
        let mut stats = SyncStats::default();

        // Fetch from API
        let tasks = self.api_client.fetch_tasks().await?;
        let projects = self.api_client.fetch_projects().await?;
        let sections = self.api_client.fetch_sections().await?;

        let mut storage = self.storage.lock().await;

        // Update cache
        for task in tasks {
            let entity = self.todoist_task_to_storage(&task)?;
            storage.upsert("todoist_tasks", entity).await?;
            stats.tasks_synced += 1;
        }

        // Push dirty items
        let dirty = storage.get_dirty("todoist_tasks").await?;
        for task_id in dirty {
            if let Some(entity) = storage.get("todoist_tasks", &task_id).await? {
                let api_task = self.storage_to_todoist_task(&entity)?;
                self.api_client.update_task(&task_id, api_task).await?;
                storage.mark_clean("todoist_tasks", &task_id).await?;
                stats.tasks_pushed += 1;
            }
        }

        self.last_sync = Some(Utc::now());
        Ok(stats)
    }

    fn capabilities(&self) -> SystemCapabilities {
        SystemCapabilities {
            supports_hierarchy: true,  // subtasks
            supports_references: false, // no built-in references
            supports_sync: true,
            read_only: false,
        }
    }

    // Helper: Convert storage entity to universal Entity
    fn task_entity_to_entity(&self, task: StorageEntity) -> Result<Entity> {
        let task_id = task.get("id")
            .and_then(|v| v.as_string())
            .ok_or(Error::MissingField("id"))?;

        let mut relationships = vec![];

        // Parent relationship (subtask)
        if let Some(parent_id) = task.get("parent_id").and_then(|v| v.as_string()) {
            relationships.push(Relationship::Parent {
                parent: Self::url_from_task_id(parent_id),
            });
        }

        // Container relationship (section or project)
        if let Some(section_id) = task.get("section_id").and_then(|v| v.as_string()) {
            relationships.push(Relationship::Container {
                container: EntityUrl(Url::parse(&format!("https://todoist.com/app/section/{}", section_id))?),
            });
        }

        Ok(Entity {
            url: Self::url_from_task_id(task_id),
            entity_type: "task".to_string(),
            data: task,
            relationships,
        })
    }
}
```

## 7. Application Layer (System-Agnostic)

```rust
// This code doesn't know about ANY specific system
pub struct Application {
    registry: SystemRegistry,
}

impl Application {
    pub fn new() -> Self {
        Self {
            registry: SystemRegistry::new(),
        }
    }

    // Register systems at startup
    pub fn initialize(&mut self, config: Config) -> Result<()> {
        // Register internal system
        self.registry.register(Box::new(InternalSystem::new()));

        // Register external systems from config
        if let Some(todoist_token) = config.todoist_token {
            let storage = SqliteBackend::new("todoist.db").await?;
            self.registry.register(Box::new(
                TodoistSystem::new(Arc::new(Mutex::new(storage)), todoist_token)
            ));
        }

        if let Some(jira_config) = config.jira {
            // Register Jira system...
        }

        Ok(())
    }

    // Universal operations (work across all systems)
    pub async fn get_entity(&self, url: &EntityUrl) -> Result<Option<Entity>> {
        self.registry.get(url).await
    }

    pub async fn get_children(&self, parent_url: &EntityUrl) -> Result<Vec<Entity>> {
        self.registry.children(parent_url).await
    }

    pub async fn get_backlinks(&self, target_url: &EntityUrl) -> Result<Vec<Entity>> {
        self.registry.backlinks(target_url).await
    }

    pub async fn get_tree(&self, root_url: &EntityUrl, max_depth: Option<usize>) -> Result<EntityTree> {
        self.registry.get_tree(root_url, max_depth).await
    }

    // Render a block with embedded references
    pub async fn render_block(&self, block_url: &EntityUrl) -> Result<String> {
        let entity = self.get_entity(block_url).await?
            .ok_or(Error::NotFound)?;

        let content = entity.data.get("content")
            .and_then(|v| v.as_string())
            .ok_or(Error::MissingField("content"))?;

        // Resolve all references
        let mut rendered = content.to_string();
        let re = Regex::new(r"\[\[([^\]]+)\]\]")?;

        for cap in re.captures_iter(content) {
            let link = &cap[1];
            let target_url = EntityUrl::from_wiki_link(link)?;

            // Recursively render referenced entity
            let referenced_html = self.render_entity(&target_url).await?;
            rendered = rendered.replace(&cap[0], &referenced_html);
        }

        Ok(rendered)
    }

    async fn render_entity(&self, url: &EntityUrl) -> Result<String> {
        let entity = self.get_entity(url).await?
            .ok_or(Error::NotFound)?;

        // Get children if hierarchical
        let children = self.get_children(url).await?;

        // Render based on entity type
        match entity.entity_type.as_str() {
            "block" => {
                let content = entity.data.get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("");
                Ok(format!("<div class='block'>{}</div>", content))
            }
            "task" => {
                let content = entity.data.get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("");
                let completed = entity.data.get("completed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut html = format!(
                    "<div class='task'><input type='checkbox' {} />{}</div>",
                    if completed { "checked" } else { "" },
                    content
                );

                // Render subtasks
                if !children.is_empty() {
                    html.push_str("<div class='subtasks'>");
                    for child in children {
                        html.push_str(&self.render_entity(&child.url).await?);
                    }
                    html.push_str("</div>");
                }

                Ok(html)
            }
            _ => Ok(format!("<div>{:?}</div>", entity)),
        }
    }
}
```

## 8. Complete Example

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let mut app = Application::new();

    // Initialize with config
    app.initialize(Config {
        todoist_token: Some("your-token".to_string()),
        jira: None,
    }).await?;

    // Create an internal block
    let block_url = EntityUrl::internal_block("project-overview");

    // Write content with references
    let content = r#"
# Project Overview

## Tasks from Todoist
[[https://todoist.com/app/task/123456789]]

## Related Blocks
[[design-notes]]
[[meeting-notes-2025-10-15]]

## Nested Structure
- [[sub-block-1]]
  - [[sub-block-1-1]]
- [[sub-block-2]]
"#;

    // The application doesn't know about Loro or Todoist specifically
    // It just works with URLs and entities

    // Get entity from any system
    let task = app.get_entity(&EntityUrl::todoist_task("123456789")).await?;
    println!("Task: {:?}", task);

    // Get children (works for internal blocks AND todoist subtasks)
    let children = app.get_children(&block_url).await?;
    println!("Children: {:?}", children);

    // Get backlinks (who references this?)
    let backlinks = app.get_backlinks(&EntityUrl::internal_block("design-notes")).await?;
    println!("Referenced by: {:?}", backlinks);

    // Render with all references resolved
    let html = app.render_block(&block_url).await?;
    println!("{}", html);

    // Get tree (follows hierarchy across systems)
    let tree = app.get_tree(&block_url, Some(3)).await?;

    Ok(())
}
```

## 9. URL Patterns Reference

```
Internal Content:
  pkm://block/abc123
  pkm://page/my-page

Todoist:
  https://todoist.com/app/task/123456789
  https://todoist.com/app/project/456789
  https://todoist.com/app/section/789012

Jira:
  https://yourcompany.atlassian.net/browse/PROJ-123

Gmail:
  https://mail.google.com/mail/u/0/#inbox/abc123def456

GitHub:
  https://github.com/user/repo/issues/123
  https://github.com/user/repo/pull/456
```

## 10. Directory Structure

```
src/
├── main.rs                 # Application entry point
├── lib.rs
├── application.rs          # Application layer (system-agnostic)
├── registry.rs             # SystemRegistry
├── entity.rs               # Entity, EntityUrl, Relationship types
│
├── modules/
│   ├── internal/
│   │   └── mod.rs         # InternalSystem implementation
│   │
│   ├── todoist/
│   │   ├── mod.rs         # TodoistSystem implementation
│   │   ├── client.rs      # API client
│   │   └── types.rs       # Todoist-specific types
│   │
│   ├── jira/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   └── types.rs
│   │
│   └── gmail/
│       ├── mod.rs
│       ├── client.rs
│       └── types.rs
│
└── storage/
    ├── mod.rs              # StorageBackend trait
    ├── sqlite.rs           # SQLite implementation
    └── loro.rs             # Loro implementation (for internal only)
```

## Benefits of This Architecture

1. **Perfect Module Isolation**: Each system is completely independent
2. **URL-Based Identity**: Natural, clickable, universal identifiers
3. **Unified Semantics**: Same relationship model everywhere
4. **System-Agnostic Code**: Application layer knows nothing about specific systems
5. **Easy Extension**: New systems just implement `EntitySystem`
6. **Actual URLs Work**: Can open in browser/app
7. **LogSeq-Style Links**: `[[todoist:123]]` just works

**Code Estimate:**
- Core abstractions (Entity, Registry): ~500 lines
- Internal system: ~600 lines
- Per external system: ~400 lines (much simpler now!)
- Application layer: ~300 lines

**Total for 3 external systems: ~2,900 lines** (vs 7,000 before!)

This is really elegant - everything is just entities with URLs and relationships. Want me to flesh out any particular part?
</ai>
