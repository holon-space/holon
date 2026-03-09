# Obsidian vs Holon Feature Comparison

Comprehensive comparison of Obsidian's 30 core plugins and 2,749 community plugins (weighted by download statistics) mapped to Holon's architecture. Generated 2026-03-25.

---

## Part I: Built-in Features (30 Core Plugins)

Obsidian ships with 30 core plugins that can be toggled on/off. Unlike LogSeq which includes PDF annotation and flashcards out of the box, Obsidian's core is deliberately minimal — a Markdown editor with linking — and relies on its plugin ecosystem for most functionality.

### Markdown Editor (Live Preview + Source Mode)

**Obsidian:** Full Markdown editor with two modes: Live Preview (WYSIWYG-like) and Source Mode (raw Markdown). Supports headings, lists, code blocks, callouts, math (MathJax), tables, and footnotes.

**Holon:** Org-mode editing in Flutter frontend. `BlockOperations` (split, indent, outdent, move) provide structural editing.

**Gap:** Holon's editor is less polished. Obsidian's editor is mature, battle-tested, and extensible. Editor UX is Holon's biggest gap by user hours spent.

### Internal Linking & Backlinks

**Obsidian core:** `[[Wikilinks]]` and standard Markdown links. **Backlinks** core plugin shows all notes linking to the current note. **Outgoing Links** core plugin shows what the current note links to.

**Holon:** Links are block relationships stored in SQL. GQL graph queries for traversal. Backlinks are a PRQL query: `from block | filter content ~= '[[target]]'`.

**Advantage Holon:** Links are queryable data, not just rendered syntax. But Holon lacks the dedicated backlinks/outgoing links UI panels.

### Graph View

**Obsidian core:** Interactive force-directed graph of all notes and their connections. Local graph (connections to current note) and global graph (entire vault). Filterable by tags, folders, search.

**Holon:** GQL provides programmatic graph traversal. No visual graph view.

**Gap:** Visual graph rendering. High user demand (see also Juggl community plugin at 115k, 3D Graph at 52k, ExcaliBrain at 288k).

### Canvas (Core Plugin — "Bases" Predecessor)

**Obsidian core:** Infinite 2D freeform canvas for arranging notes, images, web pages, and text cards. Cards can be connected with arrows. Used for visual brainstorming, planning, and mind mapping.

**Holon:** No spatial canvas.

**Gap:** Critical. Canvas is a core Obsidian feature, and Excalidraw (5.6M downloads) massively extends it. Visual/spatial thinking is the #1 demanded extension across all PKM tools.

### Bases (Core Plugin — Database Views)

**Obsidian core:** Database-like views of notes with sorting, filtering, grouping, and custom properties. Similar to Notion databases. Relatively new addition.

**Holon:** PRQL/GQL/SQL queries + render DSL (`table()`, `list()`, `columns()`). Turso IVM for live-updating views.

**Advantage Holon:** More powerful query language and incremental materialization. Bases is a simplified UI wrapper; Holon's queries are fully programmable.

### Daily Notes & Templates

**Obsidian core:** **Daily Notes** creates one note per day with optional template. **Templates** stores reusable text snippets with variable support (`{{date}}`, `{{time}}`, `{{title}}`).

**Holon:** Org journal files via OrgSyncController. Render DSL for output templating.

**Gap:** No automatic daily note creation. No input template system for new block creation.

### Search & Quick Switcher

**Obsidian core:** **Search** with regex support, path/tag/property filters. **Quick Switcher** for keyboard-based file search and creation.

**Holon:** SQL `LIKE` / PRQL `filter`. Navigation via `navigation_cursor` → `current_focus` materialized view chain.

**Gap:** No fuzzy search, no FTS5 index, no quick-switcher UI. Omnisearch community plugin (1.3M) shows massive demand for better search.

### Properties View

**Obsidian core:** Displays and manages YAML frontmatter properties. Vault-wide property browsing and type management.

**Holon:** Block properties as `HashMap<String, Value>`. Entity Type System (planned) for typed properties.

**Status:** Comparable once Entity Type System ships.

### File Management

**Obsidian core:** **Files** (hierarchical file explorer), **Bookmarks** (quick access sidebar), **File Recovery** (snapshot-based rollback at configurable intervals).

**Holon:** Document hierarchy is queryable data. Navigation history as materialized view. No file recovery — Loro CRDT handles versioning, Jujutsu/Git for file-level history.

### Note Composer

**Obsidian core:** Extract selected text into new notes. Combine existing notes. Split and merge.

**Holon:** `BlockOperations::split_block()` + `CrudOperations::create()` / `delete()`. Structural operations exist; no "extract to new document" composite operation.

**Gap:** Composite "refactor" operations (extract-to-document, merge-documents).

### Slides

**Obsidian core:** Create Markdown-based presentations from notes using `---` separators.

**Holon:** No presentation mode.

**Gap:** A `slides()` render expression could address this. Advanced Slides community plugin at 813k shows demand.

### Outline

**Obsidian core:** Sidebar showing note headers with drag-and-drop reordering of sections.

**Holon:** Block hierarchy is the outline. Depth-based rendering in Flutter. No dedicated outline sidebar panel.

### Sync & Publish (Paid Services)

**Obsidian core:** **Sync** provides encrypted cross-device sync (subscription). **Publish** hosts notes as a website (subscription).

**Holon:** Loro CRDT + Iroh P2P sync (free, decentralized, no server needed). No publish feature.

**Advantage Holon:** Free decentralized sync vs paid centralized sync. But Holon's sync is less battle-tested.

### Other Core Plugins

| Core Plugin | Holon Equivalent |
|---|---|
| Audio Recorder | No equivalent — could be MCP tool |
| Command Palette | No equivalent — Flutter/GPUI needs command palette |
| Footnotes View | Footnotes in org-mode exist; no sidebar view |
| Format Converter | OrgSyncController handles org→SQL; no general converter |
| Page Preview | No hover preview — could be render expression |
| Random Note | Trivial PRQL: `from block \| sort {random()} \| take 1` |
| Slash Commands | No equivalent — needs editor integration |
| Tags View | Tags queryable via PRQL; no dedicated sidebar |
| Unique Note Creator | Not needed — block IDs are UUIDs |
| Web Viewer | No equivalent — could embed via WebView |
| Word Count | Trivial computed property |
| Workspaces | No equivalent — needs layout persistence |

---

## Part II: Community Plugin Ecosystem (2,749 plugins)

### Download Distribution

| Bracket | Plugins | Insight |
|---|---|---|
| >= 1,000,000 | 19 | The essentials — what users truly depend on |
| >= 500,000 | 18 | Broadly adopted |
| >= 100,000 | 137 | Established, covering real needs |
| >= 50,000 | 98 | Solid niche |
| >= 10,000 | 543 | Long tail with real users |
| < 10,000 | 1,929 | Experimental or abandoned |

Top 100 plugins account for ~68% of all downloads (74M of 109M).

### 1. Query Engines & Data Views

- **Dataview** (#3, 3.8M) — SQL-like queries over vault metadata
- **Datacore** (#83, 238k) — Faster reactive query engine
- **Metadata Menu** (#81, 250k) — Manage/access note metadata
- **Meta Bind** (#63, 325k) — Inline input fields and metadata displays

**Holon:** Core strength. PRQL/GQL/SQL + Turso IVM is architecturally far more powerful than Dataview.

### 2. Task Management

- **Tasks** (#4, 3.2M) — Due dates, recurring tasks, filtering
- **Kanban** (#8, 2.2M) — Markdown-backed Kanban boards
- **Day Planner** (#28, 761k) — Time-block planning
- **Checklist** (#49, 436k) — Consolidated checklist view
- **Rollover Daily Todos** (#137, 137k)
- **CardBoard** (#115, 157k) — Kanban-style boards
- **Reminder** (#72, 277k) — System notifications
- **Time Ruler** (#232, 60k) — Drag-and-drop time ruler
- **Pomodoro Timer** (#230, 60k)
- **TaskNotes** (#107, 175k) — Calendar + pomodoro + time tracking

| Feature | Holon Status |
|---|---|
| Task state cycling | Implemented (`Cmd+Enter`) |
| Due dates, scheduling | Implemented (block properties) |
| Recurring tasks | **Gap** — needs recurrence rules + Petri Net scheduling |
| WSJF/priority ranking | Implemented (Petri Net) |
| Kanban view | **Gap** — needs render expression |
| Time blocking | **Gap** — needs calendar + drag-drop widget |
| Pomodoro timer | **Gap** — needs timer widget |
| System notifications | **Gap** — needs OS notification integration |

**Key insight:** Tasks is #4 (3.2M). Task management is the #1 functional need after basic editing.

### 3. Templates & Content Generation

- **Templater** (#2, 3.9M) — Advanced templates with JS execution
- **QuickAdd** (#12, 1.7M) — Quick note/content creation
- **Buttons** (#55, 374k) — Run commands, insert templates

**Gap:** No quick-capture / quick-add UI. No slash-command template insertion.

### 4. Calendar & Date Management

- **Calendar** (#6, 2.4M) — Calendar widget
- **Full Calendar** (#51, 396k) — Full calendar with events
- **Periodic Notes** (#29, 611k) — Daily/weekly/monthly notes
- **Natural Language Dates** (#41, 476k) — Date NLP
- **Heatmap Calendar** (#122, 151k) — Activity heatmap

**Gap:** Calendar widget and date NLP. Calendar is #6 (2.4M) — **high-priority gap**.

### 5. AI & LLM Integration

- **Copilot** (#17, 1.1M) — Chat with vault, RAG
- **Smart Connections** (#23, 856k) — AI link discovery, semantic search
- **Text Generator** (#36, 517k) — GPT-3 generation
- **Smart Composer** (#134, 139k)
- **MCP Tools** (#272, 51k) — Claude Desktop vault access

**Holon:** MCP server is this natively. The `MCP Tools` plugin (#272) is essentially what Holon already has built-in.

**Gap:** Semantic/vector search for Smart Connections-style features.

### 6. External Service Sync

- **Remotely Save** (#11, 1.7M) — S3/Dropbox/WebDAV
- **Git** (#7, 2.3M) — Version control
- **Self-hosted LiveSync** (#32, 601k) — CouchDB sync
- **Zotero Integration** (#46, 457k)
- **Todoist Sync** (#109, 166k)
- **Google Calendar** (#106, 178k)
- **Kindle Highlights** (#124, 147k)
- **Readwise Official** (#91, 210k)

**Holon:** Core strength. Architecture ready. Sync is a top-3 concern (Git 2.3M + Remotely Save 1.7M).

### 7. Visual Styling & Theming

- **Style Settings** (#9, 2.2M) — CSS variable adjustment
- **Iconize** (#10, 1.9M) — Icons everywhere
- **Minimal Theme Settings** (#13, 1.4M)
- **Banners** (#58, 349k)
- **Highlightr** (#30, 611k)

**Gap:** No parametric style system. 3 of the top 13 plugins are theming — visual customization is essential.

### 8. PDF & Document Annotation

- **Annotator** (#35, 554k) — PDF/EPUB annotation
- **PDF++** (#39, 498k) — Native PDF annotation
- **Markmind** (#37, 511k) — Mind map + PDF annotation
- **Text Extractor** (#76, 272k) — OCR

**Gap:** Significant for academic users. Better addressed by syncing from external PDF tools via MCP.

### 9. Diagrams, Canvas & Visual Thinking

- **Excalidraw** (#1, 5.6M!) — Drawing and diagrams
- **Mind Map** (#26, 786k) — Markmap mind maps
- **Advanced Canvas** (#38, 498k)
- **Mermaid Tools** (#80, 255k)
- **ExcaliBrain** (#68, 288k) — TheBrain-style mind map
- **Leaflet** (#75, 272k) — Interactive maps

**Gap:** Largest gap by user demand. Excalidraw at 5.6M is the #1 plugin. Options: embed Excalidraw/tldraw via WebView, build native drawing in Flutter, or sync from external drawing tools.

### 10. Navigation & File Management

- **Omnisearch** (#14, 1.3M) — Intelligent search
- **Homepage** (#19, 1.0M) — Custom startup page
- **Recent Files** (#20, 964k)
- **Tag Wrangler** (#21, 909k) — Tag management
- **Quick Switcher++** (#57, 363k)
- **Breadcrumbs** (#79, 258k) — Hierarchy visualization

**Gap:** Fuzzy/semantic search (FTS5 + vectors). Quick-switcher UI.

### 11. Tables & Spreadsheets

- **Advanced Tables** (#5, 2.7M) — Table editing
- **Sheet Plus** (#155, 123k) — Excel-like spreadsheets
- **Excel** (#204, 72k) — Spreadsheets in notes

**Gap:** No editable table widget. Advanced Tables at 2.7M is a top-5 need. Holon's tables are read-only query results.

### 12. Export & Publishing

- **Pandoc** (#40, 485k)
- **Advanced Slides** (#25, 813k) — Presentations
- **Webpage HTML Export** (#142, 133k)
- **Digital Garden** (#207, 70k) — Publish to web

**Holon:** Out of scope. A `slides()` render expression would address the 813k Slides demand.

### 13. Editor Enhancements

- **Outliner** (#16, 1.1M) — Workflowy/Roam-style editing
- **Editing Toolbar** (#15, 1.2M) — Formatting toolbar
- **Linter** (#24, 838k) — Format and style notes
- **Various Complements** (#45, 462k) — Auto-completion
- **Vimrc Support** (#133, 140k) — Vim keybindings
- **Longform** (#126, 146k) — Novel/screenplay writing

**Gap:** Major. Editor polish directly affects adoption. Pure frontend work.

### 14. Spaced Repetition & Learning

- **Spaced Repetition** (#44, 465k)
- **Export to Anki** (#154, 124k)

**Status:** Architecturally possible via Petri Net + FSRS. Not built.

### 15. Collaboration

- **Relay** (#104, 182k) — Real-time collaboration with live cursors

**Advantage Holon:** Loro CRDT is architecturally superior to Relay's file-level approach.

### 16. REST API & Automation

- **Local REST API** (#74, 272k)
- **Advanced URI** (#34, 558k)
- **Shell commands** (#208, 70k)

**Holon:** The MCP server IS this. Native capability.

### 17. Privacy & Security

- **Meld Encrypt** (#113, 161k)
- **Password Protection** (#271, 51k)

**Gap:** No encryption or access control.

### 18. Link Management & Graph Visualization

- **Supercharged Links** (#100, 189k)
- **Juggl** (#160, 115k) — Interactive graph
- **3D Graph** (#268, 52k)
- **Graph Analysis** (#223, 65k)

**Holon:** GQL provides native graph queries. Gap: visual graph rendering.

### 19. Maps & Geospatial

- **Leaflet** (#75, 272k) — Interactive maps
- **Map View** (#131, 141k)

**Gap:** No map/geo support. Needs `map()` render expression.

---

## Summary by User Demand

| Category | Top Downloads | Holon Status | Priority |
|---|---|---|---|
| Visual/Spatial (Excalidraw, Canvas) | 5.6M + 498k | **Major gap** | Highest demand |
| Templates & Quick Capture | 3.9M + 1.7M | **Gap** — input templates missing | High |
| Query Engine (Dataview, Bases) | 3.8M | **Core strength** — superior | Done |
| Task Management (Tasks, Kanban) | 3.2M + 2.2M | **Partially covered** — widgets missing | High |
| Tables (Advanced Tables) | 2.7M | **Gap** — editable tables missing | High |
| Calendar | 2.4M | **Gap** — calendar widget needed | High |
| Sync/Backup (Git, Remotely Save) | 2.3M + 1.7M | **In progress** — Loro/Iroh | Critical |
| Theming (Style Settings) | 2.2M | **Gap** — parametric style system | Medium |
| Search (Omnisearch) | 1.3M | **Gap** — FTS5 + vector needed | High |
| Editor UX (Outliner, Toolbar) | 1.1M + 1.2M | **Major gap** — frontend work | High |
| AI/LLM (Copilot, Smart Connections) | 1.1M + 856k | **Handled by design** — MCP | Low |
| PDF Annotation | 554k + 498k | **Significant gap** | Medium |
| External Sync (Zotero, Todoist) | 457k + 166k | **Core strength** | Per-integration |
| Export/Publishing (Slides) | 813k + 485k | **Out of scope** — slides notable | Low |
| Spaced Repetition | 465k | **Architecturally possible** | Medium |
| Collaboration (Relay) | 182k | **Advantage** — CRDT native | In progress |
| API/Automation (REST API, URI) | 558k + 272k | **Native** — MCP server | Done |
| Privacy/Encryption | 161k | **Gap** | Low |
| Maps/Geo | 272k | **Gap** | Low |

## Key Takeaways

1. **Excalidraw at 5.6M is the elephant in the room.** Visual/spatial thinking is the #1 demanded extension. Holon needs a strategy — likely embedding a drawing tool via WebView render expression.

2. **The core PKM workflow is: query + tasks + tables + calendar.** Dataview (3.8M), Tasks (3.2M), Advanced Tables (2.7M), Calendar (2.4M). Holon's query engine covers Dataview and surpasses it. The other three need visual widgets.

3. **AI plugins validate Holon's MCP-first approach.** Copilot (1.1M) and Smart Connections (856k) are popular, but MCP Tools (#272) shows the industry moving toward Holon's native architecture.

4. **Editor polish matters enormously.** Outliner (1.1M) + Editing Toolbar (1.2M) = 2.3M combined. Editing ergonomics directly affect adoption.

5. **Obsidian needs 2,749 plugins because its core is a Markdown editor.** Holon's query engine + render DSL + sync providers + MCP surface make most plugin categories either native capabilities or external MCP agents. The categories that genuinely need "widget-like" extension are: **spatial/visual tools**, **editor UX**, and **rich media widgets** (PDF, video, spreadsheet, charts).

6. **The biggest structural difference:** Obsidian's Bases core plugin is a simplified version of what Holon's PRQL + render DSL already does. Obsidian is moving toward database-backed views — arriving where Holon started.

7. **Sync is a top-3 concern** (Git 2.3M, Remotely Save 1.7M). Holon's Loro/Iroh is the right architecture but must prove reliability.
