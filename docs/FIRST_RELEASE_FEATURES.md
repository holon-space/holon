# Holon First Release: Feature Set

## The Problem with Launching Today

The internet in 2026 is hostile to new software. Every week brings another "AI-powered" note app that's really just a ChatGPT wrapper around Markdown files. People have been burned by:

- **Roam Research** (hyped, stagnated, $15/mo with declining features)
- **Notion AI** (bolted-on, slow, privacy concerns)
- **Dozens of "second brain" apps** that are just todo lists with backlinks
- **AI slop generators** that produce impressive demos but break on real workflows

The PKM community is cynical, exhausted, and vocal. A Hacker News launch with a half-baked product gets one shot — a bad first impression becomes the permanent reputation.

### What Kills New PKM Tools

1. **"It's just X with Y"** — if people can reduce it to "Obsidian but worse + some AI", it's dead on arrival
2. **Empty promises** — "will support X in the future" means nothing; ship it or don't mention it
3. **Migration cliff** — requiring users to abandon their existing tool is a non-starter
4. **Basic editing bugs** — if typing text feels wrong, nothing else matters
5. **AI-first positioning** — immediately triggers "slop" associations; structural capabilities must lead

### What Holon Must Avoid

- Do NOT lead with "AI-powered"
- Do NOT ask users to migrate their vault
- Do NOT demo features that don't work reliably
- Do NOT ship render expressions that crash or show errors
- Do NOT promise a plugin ecosystem

## The Strategy: "Superset, Not Replacement"

Holon's first release positions as a **power tool that sits on top of existing PKM setups**. The pitch:

> "Point Holon at your Obsidian vault and your Todoist account. See everything in one place. Query across both. Get ranked task lists computed across every system. Keep using your existing tools — Holon makes them better."

This means:

1. **Zero migration** — Obsidian/LogSeq vault stays as-is; Holon watches the folder
2. **Immediate value** — within 5 minutes of setup, the user sees something they couldn't see before
3. **Incremental adoption** — use Holon for querying and task ranking first, editing later
4. **No lock-in** — all data stays in the user's files; Holon's cache is disposable

## Current State Assessment

### What Works Today

| Capability | Status | Quality |
|---|---|---|
| Org-mode file sync (bidirectional) | Production | Solid — echo suppression, property preservation |
| Todoist sync (bidirectional) | Production | Solid — full CRUD + task operations |
| PRQL/GQL/SQL query engine | Production | Strong — Turso IVM, three languages |
| MCP server (query + CRUD) | Production | Full surface area |
| Petri Net / WSJF ranking | Production | Working with PBT coverage |
| Render DSL (45 registered widgets) | Production | ~30 implemented in GPUI, fewer in Flutter |
| Loro CRDT | Production | Working for block operations |
| CDC streaming (watch_ui) | Production | UiEvent enum, generation tracking |
| Block operations (indent, outdent, move, split) | Production | Backend solid |

### What Doesn't Work Yet

| Capability | Status | Blocker |
|---|---|---|
| Text editing in Flutter | Broken | editable_text renders as static text |
| Text editing in GPUI | Partial | EditorView works but limited wiring |
| Command palette | Missing | No keybinding dispatch |
| Navigation UI | Missing | focus/go_back exist but no buttons |
| Obsidian vault sync | Missing | No `holon-obsidian` crate |
| LogSeq vault sync | Missing | No `holon-logseq` crate |
| Calendar widget | Missing | No `calendar()` render expression |
| Kanban widget | Missing | No `kanban()` render expression |
| Chart widget | Missing | No `chart()` render expression |
| Search UI | Missing | Providers exist, no wiring |
| Quick capture | Missing | No input template system |

## The First Release Feature Set

### Gate: What Must Work Flawlessly

These are not features to list on a landing page. These are prerequisites. If any of these feel broken, the product is DOA.

**G1. Editing must feel right.** A user clicks a block, types text, presses Enter to create a new block, Tab to indent, Shift-Tab to outdent. No lag, no visual glitch, no lost characters. This is table stakes. (Primary target: GPUI frontend.)

**G2. Startup must be fast.** App opens in under 2 seconds. Vault sync completes in under 5 seconds for a 1,000-note vault. No splash screens, no loading spinners that last more than a moment.

**G3. No visible errors.** No `[unsupported: ...]` text. No empty panels where data should be. No panics. Render expressions that can't display their data should show nothing, not error states.

**G4. Org-mode round-trip fidelity.** User edits in Holon → org file updates correctly. User edits in Emacs → Holon updates correctly. No data loss. No property mangling.

### Tier 1: The Hook (Why Install Holon)

These are the features that make someone say "ok, that's actually useful" within the first 5 minutes.

**F1. Obsidian Vault as Data Source**

Point Holon at an Obsidian vault folder. Within seconds, all notes are queryable via PRQL. Markdown + YAML frontmatter parsed into blocks. Tags, properties, tasks, links — all accessible. File watcher for live updates.

Implementation: `holon-obsidian` crate following the `OrgSyncController` pattern. Parse Markdown + YAML frontmatter → Block entities. Watch folder. Bidirectional: write-back Markdown when blocks are mutated in Holon.

This is the single most important feature for adoption. Obsidian has the largest user base. Zero migration.

**F2. Todoist + Vault in One View**

After connecting Todoist (API key) and pointing at a vault, the user sees a unified view: all tasks from Todoist AND all TODO items from their notes, ranked by the Petri Net WSJF algorithm.

This is the "wow" moment — something no Obsidian plugin can do. Cross-system task ranking.

Requires: F1 + existing Todoist sync + Petri Net. A PRQL query across both sources rendered as a ranked list.

**F3. Live Materialized Views**

Write a PRQL query in an org source block. The result updates in real-time as data changes — no refresh button, no re-run. This is Dataview (3.8M downloads) but with proper incremental computation via Turso IVM.

Already works. Needs polish: clear visual distinction between query block and result, smooth update animations.

**F4. MCP Server (AI Access)**

Any MCP-compatible AI (Claude, etc.) can query and mutate the knowledge base. This means the user's AI assistant has full context across their vault AND their Todoist tasks.

Already works. Needs: clear setup documentation, one-command launch.

### Tier 2: Daily Driver (Why Keep Using Holon)

These make the user open Holon every day instead of just once.

**F5. Functional Editor (GPUI)**

Block-based outliner editing in GPUI (primary frontend — Flutter demoted due to FRB debugging limitations):
- Click to focus a block, type to edit
- Enter: new sibling block
- Tab / Shift-Tab: indent / outdent
- Cmd+Enter: cycle task state (TODO → DOING → DONE)
- Backspace at start: merge with previous block
- Arrow keys: navigate between blocks
- Drag-and-drop block reordering

GPUI already has EditorView with text editing + autocomplete. The wiring to block operations needs completing. GPUI also runs on Android, making it the path to mobile dogfooding.

This doesn't need to be as polished as Obsidian's editor (which has years of iteration). It needs to be correct and responsive. No cursor jumping. No lost text. Predictable behavior.

**F6. Three-Pane Layout with Navigation**

Left sidebar (document tree), main panel (content), right sidebar (contextual). Click a document to navigate. Back/forward buttons. Breadcrumb trail.

The architecture exists (navigation_cursor → current_focus materialized view). Needs UI wiring: clickable document tree, back/forward buttons, breadcrumb rendering.

**F7. Search**

Cmd+K or Cmd+P opens a quick switcher. Fuzzy search across all blocks from all sources. Type a query, arrow-key to select, Enter to navigate.

Requires: FTS5 index in Turso for fast full-text search. Search overlay UI.

**F8. Kanban Board**

`kanban(#{group_by: "task_state"})` render expression. Renders blocks grouped into columns by a property value. Drag-and-drop between columns to change state.

This replaces the #8 Obsidian plugin (2.2M downloads). Combined with cross-system queries, it can show Todoist tasks and vault TODOs on the same board.

**F9. Calendar View**

`calendar(#{date_field: "scheduled"})` render expression. Month view with blocks shown on their scheduled/deadline dates. Click a day to see all items.

Replaces the #6 Obsidian plugin (2.4M downloads).

### Tier 3: Evangelism (Why Tell Others)

These make users write blog posts and tweet about Holon.

**F10. GQL Graph Queries**

```
MATCH (task:Block {task_state: 'TODO'})-[:CHILD_OF]->(project:Block)
RETURN project.content, count(task) as open_tasks
ORDER BY open_tasks DESC
```

No Obsidian or LogSeq plugin can query the knowledge graph with a standard graph query language. This is unique.

Already works at the query level. Needs: render the results compellingly.

**F11. Cross-System Dashboard**

A single org file with PRQL queries that pull from vault + Todoist + (eventually) Calendar + JIRA. Rendered as a personalized dashboard with the render DSL:

```org
#+BEGIN_SRC holon_prql
from block
filter task_state == 'TODO'
sort {-priority, scheduled}
take 20
#+END_SRC
#+BEGIN_SRC render
list(#{item_template: row(state_toggle(), spacer(8), text(col("content")))})
#+END_SRC
```

This is Holon's equivalent of a Notion dashboard, but queryable across external systems and locally computed.

**F12. Parametric Style System**

Style variables that apply across all render expressions:

```org
#+PROPERTY: style.accent_color #4A90D9
#+PROPERTY: style.font_family Inter
#+PROPERTY: style.font_size 14
#+PROPERTY: style.spacing compact
```

Replaces Style Settings (2.2M downloads). No CSS files, no theme marketplace. Just properties.

## What Explicitly NOT to Build for V1

| Feature | Why Not for V1 Public Launch |
|---|---|
| Visual canvas / whiteboard | Massive scope. Embed Excalidraw later, or sync with external drawing tool. |
| PDF annotation | Sync from Zotero via MCP instead. |
| Spaced repetition | Natural fit but not differentiated enough to justify scope. |
| Plugin system | Render DSL + MCP + sync providers cover the need. |
| Presentation/slides mode | Low priority. Export via MCP to presentation tools. |
| Rich media playback | Low priority. Link to external players. |

### Dogfooding Track (Parallel to Public Launch)

Mobile and collaboration are **not market-driven priorities** for the first public release, but they are **essential for the developer's own daily use**. Since Holon's architecture already supports these (Loro CRDT + Iroh P2P, Flutter cross-platform), they are developed in parallel as a dogfooding track:

| Feature | Rationale | Architecture Status |
|---|---|---|
| **Mobile app (GPUI on Android)** | Daily capture and review on the go; can't rely on Todoist proxy for everything | GPUI proven to run on Android; Loro + Turso run on mobile |
| **Multi-device sync** | Using Holon on laptop + phone + desktop requires it | Iroh P2P sync designed for this; no server needed |
| **Collaboration** | Sharing task context with family/team | Loro CRDT enables real-time collaboration natively |

These ship when they're ready for one user (the developer), not when they're ready for public marketing. They may be available in V1 but not promoted — underpromise, overdeliver.

## Priority Matrix

Based on implementation effort, user impact, and what already exists:

| Priority | Feature | Effort | Impact | Depends On |
|---|---|---|---|---|
| **P0** | G1-G4 (gates) | Medium | Existential | — |
| **P1** | F1 (Obsidian vault sync) | Medium | Highest | holon-obsidian crate |
| **P1** | F5 (Functional editor) | High | Highest | Flutter/GPUI editor wiring |
| **P1** | F2 (Unified task view) | Low | High | F1 + existing infra |
| **P2** | F6 (Navigation + layout) | Medium | High | UI wiring |
| **P2** | F7 (Search) | Medium | High | FTS5 + overlay UI |
| **P2** | F3 (Live materialized views) | Low | Medium | Polish existing |
| **P2** | F8 (Kanban) | Medium | High | New render expression |
| **P3** | F9 (Calendar) | Medium | High | New render expression |
| **P3** | F4 (MCP server) | Low | Medium | Documentation only |
| **P3** | F10 (GQL queries) | Low | Medium | Already works |
| **P3** | F11 (Dashboard) | Low | Medium | F1 + F2 + render polish |
| **P4** | F12 (Parametric styles) | Medium | Medium | Style variable system |

## The Demo Script (What to Show on Launch Day)

1. **0:00** — Open Holon. Point at existing Obsidian vault (~500 notes). Watch it index in 3 seconds.
2. **0:15** — Three-pane view appears. Browse the document tree. Click into a note. Edit text. Create a task.
3. **0:30** — Connect Todoist (paste API key). Tasks appear alongside vault TODOs.
4. **0:45** — Open the unified task ranking view. "Here are your tasks across both systems, ranked by urgency × value."
5. **1:00** — Write a PRQL query: `from block | filter task_state == 'TODO' | sort {-priority}`. Watch the result update live.
6. **1:15** — Switch to Kanban view of the same query. Drag a task from TODO to DOING. Watch it update in both Holon and Todoist.
7. **1:30** — "Everything you just saw works offline. Your data stays on your machine. No account needed. Your Obsidian vault is untouched."

Total: 90 seconds. No AI mentioned. No buzzwords. Just demonstrate capability.

## The Tagline

> **"One view across all your systems. No migration. No subscription. No AI hype."**

The anti-AI-slop positioning is itself a differentiator in 2026. Lead with structure, not magic.
