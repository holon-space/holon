# LogSeq vs Holon Feature Comparison

Comprehensive comparison of LogSeq's built-in features and 486 marketplace plugins mapped to Holon's architecture. Generated 2026-03-25.

---

## Part I: Built-in Features

### Outliner & Block Model

**LogSeq:** Block-based outliner where every bullet point is an addressable block with a unique ID. Blocks can be nested, referenced (`((block-id))`), and embedded. Child blocks inherit properties from parents. Supports both Markdown and Org-mode file formats.

**Holon:** Same block-based model stored in Turso (SQLite). Blocks have `id`, `parent_id`, `sort_key` (fractional index), `depth`, `content`, and typed properties. `BlockOperations` trait provides indent, outdent, move, split, move_up, move_down. Block references are expressible as PRQL/GQL queries rather than a special syntax.

**Advantage Holon:** Blocks are queryable SQL rows, not just an outliner data structure. Any SQL/PRQL/GQL query can traverse, filter, and aggregate blocks. LogSeq's Datalog queries are powerful but less accessible than SQL.

### Bidirectional Linking & Graph View

**LogSeq:** Wiki-style `[[page links]]` and `#hashtags` with automatic backlink tracking. Graph view visualizes page interconnections.

**Holon:** Links are block relationships (parent_id, references via properties). GQL graph queries provide programmatic traversal: `MATCH (a)-[:LINKS_TO]->(b) RETURN a, b`. No visual graph view yet.

**Advantage LogSeq:** Built-in visual graph view. Holon has the query power but lacks the visualization.

**Gap:** Visual graph render expression (`graph()`).

### Journals / Daily Notes

**LogSeq:** Automatic daily journal pages (`YYYY_MM_DD` naming). Central to the workflow — the default landing page is today's journal.

**Holon:** Org journal files synced via OrgSyncController. No automatic daily page creation or journal-first navigation.

**Gap:** Journal-first navigation mode, automatic daily note creation.

### Task Management

**LogSeq built-in:** TODO/DOING/DONE states with keyboard shortcuts. Priorities (`[#A]`, `[#B]`, `[#C]`). Deadlines and scheduled dates with date picker.

**Holon:** `TaskOperations::set_state()` + `Cmd+Enter` keybinding. `scheduled`/`deadline` as block properties. Petri Net materialization for WSJF ranking. Entity Type System (planned) for custom workflow states.

**Advantage Holon:** WSJF/Petri Net ranking is more sophisticated than any LogSeq built-in or plugin. Holon can compute optimal task ordering; LogSeq only stores state.

### Query System

**LogSeq built-in:** Datalog queries (ClojureScript) and "simple queries" for filtering blocks. Powerful but requires learning Datalog syntax.

**Holon:** PRQL (primary), GQL (ISO/IEC 39075 graph queries), and raw SQL. All compile to SQL executed by Turso with incremental materialized views (IVM) for live-updating results.

**Advantage Holon:** Three query languages vs one. IVM means queries update in real-time without re-execution. SQL is more widely known than Datalog.

### Templates & Macros

**LogSeq built-in:** Reusable content structures with dynamic variables (`<%today%>`, `<%time%>`) and macro expansion. Templates can include/exclude parent blocks.

**Holon:** Render DSL (`RenderExpr`) handles output templating. No input templating system (creating new blocks from a template).

**Gap:** Input templates — quick-capture with template expansion.

### Whiteboards / Spatial Canvas

**LogSeq built-in:** Spatial canvas for visual thinking, combining knowledge graph elements with freeform drawing. Nodes on the whiteboard can link to blocks/pages. (Currently disabled in DB version, re-enabling planned.)

**Holon:** No spatial canvas or whiteboard.

**Gap:** Major. Spatial thinking is a fundamental capability. See Obsidian comparison — Excalidraw is the #1 most downloaded plugin across all PKM tools.

### PDF Annotation

**LogSeq built-in:** Highlight and annotate PDF documents directly. Annotations are stored as blocks linked to PDF positions. DB version adds annotation tags for cross-PDF views.

**Holon:** No PDF viewer or annotation system.

**Gap:** Significant for academic/research users. Could be addressed by treating a PDF tool (Zotero, PDF Expert) as an external system synced via MCP.

### Flashcards & Spaced Repetition

**LogSeq built-in:** Card-based learning with spaced repetition scheduling. Cloze deletions. Re-implemented with new algorithm in DB version.

**Holon:** No built-in spaced repetition. Architecturally natural fit: FSRS-6 review data as block properties + Petri Net scheduling for review timing + `flashcard()` render expression.

**Gap:** Not built, but the architecture supports it cleanly.

### Page Properties & Types

**LogSeq built-in:** Typed metadata system with validation for text, numbers, dates, URLs, and node references. DB version adds a full property type system.

**Holon:** Block properties stored as `HashMap<String, Value>`. Entity Type System (planned) adds `TypeDefinition` with `FieldLifetime` for typed properties from YAML definitions.

**Status:** Comparable once Entity Type System ships. Currently less structured than LogSeq DB version.

### Media Embedding

**LogSeq built-in:** Audio, video, image embedding using Markdown, HTML, and Hiccup syntax. Local files auto-organized in `assets/`. YouTube, Vimeo, etc. supported.

**Holon:** Blocks can reference media files. No rich media player widgets.

**Gap:** Media playback widgets in the frontend.

### Search

**LogSeq built-in:** Full-text search with page navigation and filtering.

**Holon:** SQL `LIKE` / PRQL `filter` for text search. No fuzzy search or full-text search index.

**Gap:** FTS5 index for fast full-text search. Eventually vector embeddings for semantic search.

### File Format & Storage

**LogSeq:** Plain text Markdown or Org-mode files (file graph). SQLite-backed unified storage (DB graph, beta). Transitioning from files to DB.

**Holon:** Org-mode files as source of truth, synced bidirectionally to Turso (SQLite) cache via OrgSyncController. Loro CRDT documents for collaboration. Holon treats files as an external system, not as the storage format.

**Advantage Holon:** True separation of concerns — files are a sync target, not the database. Enables real-time queries, materialized views, and CDC streams that file-based storage cannot support.

### Real-Time Collaboration

**LogSeq:** RTC (Real-Time Collaboration) in alpha for DB version.

**Holon:** Loro CRDT documents enable real-time collaboration. Iroh P2P sync for decentralized sync without server infrastructure.

**Advantage Holon:** CRDT-native architecture vs LogSeq's bolt-on RTC.

---

## Part II: Plugin Ecosystem (486 plugins)

### 1. Themes & Visual Styling (~80 plugins)

*gruvbox, dracula, catppuccin, rose-pine, nord, e-ink, bujo, flexoki, monokai, arc, github, tufte, paper, etc.*

**Holon approach:** Rendering is data-driven via `#+BEGIN_SRC render` blocks with Rhai expressions (`table()`, `columns()`, `list()`, `block_ref()`). Styling is structural, not CSS-injected.

**Gap:** No theming/parametric style system yet. Holon shouldn't need 80 theme plugins — the render DSL could parametrize appearance directly.

### 2. External Service Sync (~60 plugins)

*Todoist, Google Tasks, Jira, Trello, TickTick, OmniFocus, Habitica, GitHub Issues, OpenProject, Readwise, Hypothesis, Zotero, Raindrop, Wallabag, Cubox, Flomo, WeRead, Kindle, Calibre, Spotify, Linkwarden, Nostr, Telegram, RSS, Google/iCloud/Outlook Calendar*

**Holon approach:** Core architectural strength. `SyncProvider` + `QueryableCache<T>` + `OperationProvider` give true bidirectional sync. MCP client bridges to any MCP-compatible service.

**Status:** Todoist and org-mode implemented. Architecture ready for all others.

### 3. AI/LLM Integration (~20 plugins)

*ChatGPT, GPT-3/4, Azure OpenAI, Ollama, Claude, AI auto-tags, AI query, AI summarization, Composer (RAG+LiteLLM)*

**Holon approach:** MCP server exposes full query surface. These don't need to be plugins — they become MCP client agents.

### 4. Task Management & Workflows (~30 plugins)

*Custom workflows, Kanban boards, habit trackers, Pomodoro timers, deadline countdowns, daily TODO transfer*

| Feature | Holon Status |
|---|---|
| Custom workflows | Planned via Entity Type System |
| WSJF/priority ranking | Implemented (Petri Net) |
| Kanban | **Gap** — needs render expression |
| Pomodoro/timer | **Gap** — needs timer widget |
| Habit tracking | Properties + PRQL + heatmap render expression |

### 5. Query & Data Visualization (~15 plugins)

*ECharts, Vega-Lite, property visualizer, graph analysis, query builder, table rendering, heatmaps*

**Holon:** Architecturally superior (PRQL/GQL/SQL + IVM). Missing: chart/graph render expressions.

### 6. Calendar & Date Management (~15 plugins)

*Calendar views, agenda, date NLP, milestone tracking, weekly/daily templates*

**Gap:** Calendar widget, date NLP parsing.

### 7. Export & Publishing (~20 plugins)

*PDF, Hugo, WordPress, Pandoc, Markdown, HTML, Slack format, CSV*

**Holon:** Out of scope — MCP tools can serve as export surface.

### 8. Diagrams & Visual Content (~15 plugins)

*Excalidraw, Draw.io, Mermaid, PlantUML, D2, mind maps, freehand sketching*

**Gap:** No diagram rendering. Needs diagram render expressions + embedded renderers.

### 9. PDF & Document Annotation (~10 plugins)

*PDF extraction, PDF navigation, ask-pdf, formula OCR, image OCR*

**Gap:** No PDF viewer beyond what LogSeq already provides built-in.

### 10. Editor Enhancements (~25 plugins)

*VIM mode, smart typing, block navigation, find-and-replace, RTL/BiDi, long-form editing, typewriter mode*

**Gap:** Major frontend work. Editor UX is the biggest gap category.

### 11. Block & Page Utilities (~20 plugins)

*Block-to-page, page merge, orphan cleaner, block sorting, random note, breadcrumbs*

**Status:** Mostly trivially expressible as PRQL queries or `BlockOperations` compositions.

### 12. Privacy & Security (~5 plugins)

**Gap:** No encryption or access control.

### 13. Spaced Repetition Plugins (~5 plugins)

*Anki sync, Mochi sync, vocabulary cards*

**Status:** Architecture supports it (Petri Net + FSRS), not built.

### 14. External Content Embedding (~10 plugins)

*YouTube captions, web archiving, link previews, Figma, spreadsheets, RSS*

**Status:** Partially covered via FDW + MCP. Rich embeds need frontend widgets.

---

## Summary

| Category | LogSeq Built-in | Plugins | Holon Status |
|---|---|---|---|
| Outliner / Block Model | Block-based with references | — | **Parity** + SQL queryability |
| Bidirectional Links | Wiki links + backlinks | — | **Parity** (links as data) — graph view **gap** |
| Graph View | Built-in | — | **Gap** — query power without visualization |
| Journals | Daily pages, default view | — | **Gap** — no journal-first mode |
| Task Management | TODO/DOING/DONE, priorities | ~30 | **Advantage** — Petri Net WSJF; visual widgets missing |
| Query System | Datalog + simple queries | ~15 | **Advantage** — PRQL/GQL/SQL + IVM |
| Templates | Dynamic variables, macros | — | **Gap** — output templates only, no input templates |
| Whiteboards | Spatial canvas | ~15 | **Major gap** |
| PDF Annotation | Built-in viewer + highlights | ~10 | **Significant gap** |
| Flashcards | Built-in spaced repetition | ~5 | **Gap** — architecturally ready |
| Properties/Types | Typed metadata system | — | **Gap** until Entity Type System ships |
| Media | Audio/video/image embedding | ~10 | **Gap** — no media player widgets |
| Search | Full-text search | — | **Gap** — no FTS5 index |
| Collaboration | RTC (alpha) | — | **Advantage** — CRDT native (Loro + Iroh) |
| Storage | Files → DB transition | — | **Advantage** — files are sync target, not storage |
| Themes/Styling | CSS customization | ~80 | **Gap** — need parametric style system |
| External Sync | — | ~60 | **Core strength** |
| AI/LLM | — | ~20 | **Handled by design** — MCP surface |
| Editor UX | Basic outliner editing | ~25 | **Major gap** |
| Export/Publishing | — | ~20 | **Out of scope** |
| Privacy | — | ~5 | **Gap** |

## Key Takeaways

1. **LogSeq ships more built-in than Obsidian** — PDF annotation, flashcards, whiteboards, and task management are all core features. This raises the bar for Holon's built-in feature set.

2. **Holon's query engine + IVM is a genuine architectural advantage** over LogSeq's Datalog. Three languages (PRQL/GQL/SQL), incremental materialized views, and CDC streams are capabilities LogSeq cannot match.

3. **The whiteboard gap is critical.** LogSeq has it built-in, Obsidian's #1 plugin is Excalidraw. Spatial thinking is clearly a must-have for PKM tools.

4. **LogSeq is transitioning from files to DB** — exactly what Holon already does (files as sync target, SQL as query engine). This validates Holon's architectural bet but means LogSeq is catching up.

5. **Collaboration is Holon's next differentiator.** LogSeq's RTC is alpha; Holon's Loro CRDT + Iroh P2P is architecturally more mature.

6. **~160 of 486 plugins are pure themes** — a parametric style system in Holon's render DSL replaces this entire category.

7. **Holon doesn't need a plugin system** for most categories:
   - Data integrations → sync providers (crates or MCP bridges)
   - AI features → MCP client agents
   - Visualizations → render expressions
   - Queries → PRQL/GQL/SQL
   - Only new frontend widgets truly need "plugin-like" extension
