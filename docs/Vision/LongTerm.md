# Holon: The Integral Workspace

**Website**: [holon.space](https://holon.space)

> *"Reality is not composed of things or processes, but of holons."*
> — Ken Wilber

## The Name

**Holon** (from Greek *holos*, "whole") is a concept coined by Arthur Koestler and central to Ken Wilber's Integral Theory. A holon is something that is simultaneously **a whole in itself** and **a part of a larger whole**.

This perfectly captures what we're building:
- Each task, note, or project is complete in itself
- Yet each is also part of larger contexts (projects → life areas → your whole life)
- The app itself integrates multiple systems into one unified whole
- You, the user, are a holon: an individual who is also part of teams, communities, and humanity

---

## Vision Statement

Holon is an **integral workspace** that unifies your professional and personal life into a coherent whole, enabling you to achieve **flow states** by eliminating the cognitive overhead of fragmented tools.

It's not just a productivity app. It's a **trust and flow system** that happens to use productivity data.

Holon starts as a work instrument but the architecture doesn't impose that boundary. The Petri Net's token type system (Person, Organization, Document, Monetary, Knowledge, Resource) and the entity graph with confirmed edges are general enough to encode an entire life — professional history, relationships, health data, financial transactions, media consumption. We don't need to ship a life ontology on day one, but we should never make architectural decisions that prevent growing into one. The token type system is extensible by design; the user decides how much of their life to model.

### The Problem We Solve

Modern knowledge workers suffer from **tool fragmentation**:
- Tasks in Todoist, JIRA, Linear, Asana
- Notes in Notion, Obsidian, LogSeq
- Calendar in Google Calendar, Outlook
- Email in Gmail, Outlook
- Files in Google Drive, Dropbox

This fragmentation creates:
1. **Cognitive overhead**: Constantly switching contexts, mentally synthesizing what matters
2. **The nagging feeling**: "Am I forgetting something important?"
3. **Broken flow**: Can't focus deeply because you don't trust the system
4. **Lost connections**: Related items across systems never get linked

### Our Solution

Holon integrates all your systems into a **unified view** where:
- You see everything that matters in one place
- You trust that nothing is forgotten
- You can focus on the present task with complete confidence
- AI helps you see patterns and connections you'd miss

---

## Philosophical Foundation

Holon is built on principles from Integral Theory, Spiral Dynamics, and flow psychology.

### The Five Paths (Ken Wilber's Integral Life Practice)

We design Holon to support growth across all dimensions:

| Path | What It Means | How Holon Supports It |
|------|---------------|----------------------|
| **Waking Up** | Present-moment awareness, flow states | Focus mode, distraction-free deep work, flow metrics |
| **Growing Up** | Expanding perspective and capability | Pattern recognition, growth tracking, behavioral insights |
| **Opening Up** | Developing multiple intelligences | Multiple views of same data, aesthetic customization |
| **Cleaning Up** | Integrating shadow, facing what we avoid | Surfacing avoided tasks, gentle prompts about resistance |
| **Showing Up** | Embodying development in daily action | Commitment tracking, accountability, real-world execution |

### Spiral Dynamics Adaptation

Different users (or the same user at different times) operate from different value systems. Holon adapts:

| Value System | What They Need | Holon Adaptation |
|--------------|----------------|------------------|
| **Purple** (Belonging) | Connection, shared rituals | Team dashboards, shared celebrations |
| **Red** (Power) | Quick wins, visible progress | Achievement tracking, streak counters |
| **Blue** (Order) | Structure, clear processes | Templates, workflows, checklists |
| **Orange** (Achievement) | Efficiency, optimization | Analytics, productivity metrics, time tracking |
| **Green** (Community) | Collaboration, meaning | Shared projects, purpose alignment |
| **Yellow** (Integration) | Flexibility, systems view | Custom views, meta-awareness, adaptable UI |
| **Turquoise** (Holistic) | Global impact, sustainability | Long-term tracking, ripple effects |

### The Flow State Goal

The ultimate purpose of Holon is to help users achieve **flow** — the state of complete immersion where:
- You lose track of time
- Work feels effortless
- You're operating at peak performance
- There's no anxiety about what you're missing

Flow requires **trust**. You must trust that:
1. Nothing important is being forgotten
2. What you're working on is the right thing
3. The information you need is accessible

Holon is designed to build and maintain this trust.

---

## Core Experience Model

Holon operates in three modes that match how humans actually work:

```
┌─────────────────────────────────────────────────────────────────┐
│                         HOLON                                   │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐       │
│  │    CAPTURE    │  │    ORIENT     │  │     FLOW      │       │
│  │               │  │               │  │               │       │
│  │  Quick input  │  │  Big picture  │  │  Deep focus   │       │
│  │  On the go    │  │  What matters │  │  Present task │       │
│  │  Get it out   │  │  Daily/weekly │  │  Distraction  │       │
│  │  of my head   │  │  reviews      │  │  free         │       │
│  └───────────────┘  └───────────────┘  └───────────────┘       │
│                                                                 │
│  "I just thought    "Show me the      "I know exactly          │
│   of something"      whole picture"    what to do next"        │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Capture Mode
- Quick task/note entry (mobile-optimized)
- Voice capture while walking
- Forward emails with one click
- Inbox that processes to zero
- Like Todoist's quick add, but universal

### Orient Mode
- Daily orientation: "What does today look like?"
- Weekly review: comprehensive synthesis
- The Watcher Dashboard: nothing falls through cracks
- Cross-system visibility
- Like LogSeq's journal + GTD review

### Flow Mode
- Single task focus
- Relevant context surfaced automatically
- Distractions hidden
- Progress visibility
- Like deep work + Pomodoro, but smarter

---

## The Trust Architecture

### Problem 1: "What Should I Do?" Anxiety

**Current state**: Check Todoist, then JIRA, then email, then calendar, mentally synthesizing priorities.

**Holon solution**: Open the app, immediately see what to work on, trust it completely.

```
┌─────────────────────────────────────────────────────────────────┐
│  RIGHT NOW: Focus on [JIRA-456: API Authentication]            │
│                                                                 │
│  Why this? ─────────────────────────────────────────────────    │
│  • Highest impact task in current sprint                        │
│  • 2-hour focus block available before next meeting             │
│  • All dependencies resolved as of 9am                          │
│  • Aligns with Q1 priority: "Ship v2.0"                         │
│                                                                 │
│  Context you might need: ────────────────────────────────────   │
│  • [Notes from security review] (3 days ago)                    │
│  • [Email thread with Sarah about edge cases]                   │
│  • [Related Todoist task: Update auth docs]                     │
│                                                                 │
│  [Start Focus Session]  [Show alternatives]  [Why this?]        │
└─────────────────────────────────────────────────────────────────┘
```

### Problem 2: "Am I Forgetting Something?" Nagging

**Holon solution**: The Watcher Dashboard

A daily/weekly review mode where AI synthesizes:
- Everything that came in (emails, tasks, messages)
- Everything that changed (status updates, calendar changes)
- Everything at risk (deadlines, commitments, dependencies)
- Everything you said you'd do but haven't

Creates the **"empty inbox" feeling** — not that there's nothing to do, but that everything is accounted for.

### Problem 3: Context Switching Costs

**Holon solution**: Seamless Context Bundles

When working on "Project X", you see:
- All tasks related to X (from any system)
- All calendar events for X
- All communications about X
- All notes about X

Not as separate panels, but as a **unified view** where the source system is just metadata.

---

## AI as Participatory Prosthesis

AI in Holon isn't just an optimizer — it's an **externalized part of your awareness** that sees what you can't see because you're focused elsewhere.

Clark and Chalmers' extended mind thesis (1998) argues that cognitive processes extend beyond skull and skin into external tools. But most "second brain" tools are **passive prostheses** — they store and retrieve information on request, like Otto's notebook. Holon aspires to be a **participatory prosthesis**: a tool that takes initiative in the formation of new connections while preserving human authority over which connections become part of the knowledge structure.

A system qualifies as a participatory prosthesis when it meets four criteria:

1. **Domain model**: Typed entities and typed relationships, not merely stored content
2. **Proactive suggestion**: Unsolicited proposals based on pattern recognition across the domain model
3. **Constitutive confirmation**: User confirmation alters the system's knowledge state — not passive display but a meaning-making act
4. **Bidirectional adaptation**: The system shapes the user (through what it surfaces) AND the user shapes the system (through what they confirm)

This framing sharpens the difference between Holon and note-taking tools. Obsidian has a partial domain model (backlinks) but no proactive suggestion. ChatGPT Memory has partial proactive behavior but no domain model. Recommendation algorithms (Netflix, Spotify) have proactive suggestion but no constitutive confirmation — the user's click is consumption, not knowledge construction. Holon aims to satisfy all four criteria.

### Three AI Roles

#### 1. The Watcher (Awareness)
- Continuously monitors all your systems
- Notices patterns you can't see
- Detects when reality diverges from intention
- **Answers**: "What am I not seeing?"

#### 2. The Integrator (Wholeness)
- Proposes typed relationships between entities for human confirmation
- Surfaces relevant context when you need it
- Creates the "unified field" view through confirmed edges, not just search results
- **Answers**: "What else matters for this?"

The Integrator's primary interaction is **confirmation-driven edge creation**: it proposes links, the user confirms or rejects at System 1 speed (1-2 seconds). Each confirmed edge makes the graph denser without adding nodes, and denser graphs produce better future proposals — a compounding flywheel. See [AI.md](AI.md) §The Integrator.

#### 3. The Guide (Growth)
- Tracks your patterns over time
- Notices where you're stuck or avoiding
- Gently surfaces uncomfortable truths
- **Answers**: "What am I avoiding?"

### AI Features Mapped to Integral Paths

| Integral Path | AI Capability | Example |
|---------------|---------------|---------|
| **Waking Up** | Present-moment focus support | "You've been in reactive mode for 3 hours. Pause and review priorities?" |
| **Growing Up** | Pattern recognition over time | "You consistently underestimate tasks involving X. Adjust estimates?" |
| **Opening Up** | Multi-perspective views, confirmation-driven linking | Show same project from different stakeholder viewpoints; confirm cross-domain edges that bridge previously disconnected knowledge clusters |
| **Cleaning Up** | Obstacle identification | "You've postponed this 7 times. Is it a) too big b) unclear c) uncomfortable?" |
| **Showing Up** | Accountability tracking | "You committed to X. Here's your progress." |

### AI Trust Ladder

AI earns autonomy through demonstrated competence:

1. **Passive**: Answers when asked (starting point)
2. **Advisory**: Suggests, you decide (builds trust)
3. **Agentic**: Takes actions with permission (earned trust)
4. **Autonomous**: Acts within defined bounds (full trust)

Users progress through these levels as they experience AI accuracy. The system never assumes trust — it earns it.

### Shadow Work (Cleaning Up) — Done Right

Not motivation-trainer platitudes. Practical help overcoming obstacles:

```
┌─────────────────────────────────────────────────────────────────┐
│  📋 Task: "Write performance review for Alex"                   │
│  ⚠️  Postponed 7 times over 3 weeks                             │
│                                                                 │
│  This task seems stuck. What's blocking you?                    │
│                                                                 │
│  [ ] It's too big → Let's break it down together                │
│  [ ] It's unclear → Let's clarify what "done" looks like        │
│  [ ] It's uncomfortable → Let's make it less daunting           │
│  [ ] Wrong time → Reschedule to a better slot                   │
│  [ ] Shouldn't be mine → Delegate or decline                    │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## Technical Architecture

### The Holon Data Model

Everything in Holon is a **holon** — simultaneously whole and part:

```rust
pub trait Holon {
    fn id(&self) -> HolonId;
    fn title(&self) -> &str;

    // Wholeness: this item's own properties
    fn properties(&self) -> &Properties;

    // Partness: what larger wholes contain this?
    fn parents(&self) -> Vec<HolonId>;

    // Wholeness: what parts does this contain?
    fn children(&self) -> Vec<HolonId>;

    // Cross-system identity
    fn source(&self) -> Source;  // Native, Todoist, JIRA, etc.
    fn external_id(&self) -> Option<ExternalId>;

    // For AI
    fn embeddings(&self) -> Option<&[f32]>;
}
```

### Hybrid Sync Architecture

```
┌─────────────────────────────────────────┐
│       UNIFIED VIEW LAYER (Flutter)      │
│   Merged view + sync status indicators  │
└────────────────┬────────────────────────┘
                 │
         ┌───────┴────────┐
         │                │
┌────────▼───────┐ ┌──────▼────────────┐
│  OWNED DATA    │ │  THIRD-PARTY      │
│                │ │  SHADOW LAYER     │
├────────────────┤ ├───────────────────┤
│ Loro CRDT      │ │ • Local cache     │
│                │ │ • Operation log   │
│ Source of      │ │ • Reconciliation  │
│ truth for      │ │                   │
│ native items   │ │ Eventually        │
│                │ │ consistent        │
└────────────────┘ └───────────────────┘
```

**Key principles**:
1. Native Holon data uses Loro CRDT (true offline-first, multi-device sync)
2. Third-party data is cached locally but source of truth remains external
3. Offline changes queue as operations to replay when online
4. AI-powered conflict resolution when changes clash

### AI Stack

```
┌─────────────────────────────────────────────────────────────────┐
│                    UI Layer (Flutter)                           │
│         Focus Mode, Orient Dashboard, Capture Input             │
└────────────────────────┬────────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────────┐
│                   AI Services (Rust)                            │
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │   Watcher    │  │  Integrator  │  │    Guide     │          │
│  │              │  │              │  │              │          │
│  │ • Monitoring │  │ • Linking    │  │ • Patterns   │          │
│  │ • Alerts     │  │ • Context    │  │ • Insights   │          │
│  │ • Synthesis  │  │ • Search     │  │ • Growth     │          │
│  └──────────────┘  └──────────────┘  └──────────────┘          │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  Foundation: Embeddings (local) + LLM (hybrid/optional)  │  │
│  └──────────────────────────────────────────────────────────┘  │
└────────────────────────┬────────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────────┐
│                  Unified Data Layer                             │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Holon Store: CRDT + Shadow Cache + Links + Embeddings  │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Pattern Logs: Conflicts, Behaviors, User Corrections   │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Privacy Model

Three deployment options:

1. **Fully Local** (Maximum Privacy)
   - All AI on-device (GGUF models via llama.cpp)
   - Zero cloud dependency
   - Target: Privacy-conscious users, enterprises

2. **Hybrid** (Recommended)
   - Local: Embeddings, search, classification
   - Cloud: Complex reasoning (opt-in, user controls)
   - Target: Most users

3. **Self-Hosted**
   - User runs own LLM server (Ollama, vLLM)
   - Holon connects to user's infrastructure
   - Target: Technical users, teams

---

## Platform Strategy

### Primary Contexts

1. **Deep Work at Desk** (Desktop — macOS, Windows, Linux)
   - Full outliner experience
   - All views and features
   - Keyboard-driven power user mode

2. **Capture on Mobile** (iOS, Android)
   - Quick task/note entry
   - Voice capture
   - Email forwarding
   - Today view

3. **Execute on Mobile**
   - Today's tasks
   - Focus timer
   - Quick context while away from desk

4. **Walking/Brainstorming** (Mobile + Voice)
   - Voice notes
   - Add tasks to projects
   - Dictate ideas

### Technology Stack

- **Core**: Rust (performance, safety, cross-platform)
- **UI**: Flutter (single codebase, native feel)
- **Desktop**: Tauri (lightweight, secure)
- **Database**: Loro CRDT + Turso (embedded SQLite)
- **Search**: Tantivy (fast full-text search)
- **AI**: sentence-transformers (embeddings) + optional LLM

---

## Development Philosophy

### Individual First, Team Later

1. **Eat your own dogfood**: Build for individual use, use it daily
2. **Make it great for one person** before adding collaboration
3. **Team features build on individual excellence**
4. Each person's Holon should be powerful standalone

### Right Tool for the Job

Holon doesn't replace specialized tools — it **integrates** them:
- Todoist is great for quick task capture → integrate it
- JIRA is great for sprint management → integrate it
- Calendar is great for time blocking → integrate it

Let each tool do what it does best. Holon provides the **unified view**.

### Progressive Trust

Features earn their place through demonstrated value:
- Start simple, prove usefulness
- Add complexity only when justified
- AI suggestions start conservative, grow bolder with accuracy
- Never overwhelm the user

---

## Roadmap Overview

### Phase 1: Core Outliner
- Loro-based outliner with blocks, links, backlinks
- Local-only, no integrations
- Basic task visualization
- Full-text search
- Desktop + mobile
- **Goal**: Usable as LogSeq alternative

### Phase 2: First Integration (Todoist)
- Prove hybrid sync architecture
- Conflict resolution
- Offline queue
- **Goal**: Todoist feels native in Holon

### Phase 3: Multiple Integrations
- JIRA, Linear, Calendar
- Unified task type with extensions
- Cross-system search
- **Goal**: Work across systems seamlessly

### Phase 4: AI Foundation
- Local embeddings
- Semantic search
- Entity linking (manual → automatic)
- Pattern logging
- **Goal**: AI infrastructure ready

### Phase 5: AI Features
- The Watcher (monitoring, alerts)
- The Integrator (context, links)
- The Guide (patterns, insights)
- **Goal**: AI provides daily value

### Phase 6: Flow Optimization
- Focus mode with context bundles
- Orient dashboard
- Review workflows
- Obstacle identification
- **Goal**: Users achieve flow states regularly

### Phase 7: Team Features
- Shared views
- Collaborative editing
- Team dashboards
- **Goal**: Teams leverage individual excellence

---

## Success Metrics

### Flow Metrics
- Time spent in focus mode
- Frequency of context switches
- User-reported flow states (sampling)

### Trust Metrics
- How often users check other apps (should decrease)
- Review completion rate
- Inbox zero frequency

### AI Metrics
- Suggestion acceptance rate
- Correction rate (should decrease over time)
- Insights acted upon

### Growth Metrics (Long-term)
- Daily active users
- Retention (30-day, 90-day)
- NPS score
- "Can't switch" responses in surveys

---

## Potential Extensions / Ideas

These ideas are inspired by the [Functional Systems Paradigm](https://strategicdesign.substack.com/p/the-functional-systems-paradigm) (FSP), which shares significant conceptual overlap with Holon's architecture while offering complementary formalizations.

### Overlap with Holon

FSP's core axioms align with Holon's existing design:

| FSP Concept | Holon Implementation |
|-------------|---------------------|
| "Function Precedes Form" | Trait-based types (`TaskEntity`, `BlockEntity`) define behavior, not content |
| "One canonical location, multiple projections" | Third-party items appear in Context Bundles, unified search, project views |
| "Relationships Carry Meaning" | Graph structure with backlinks, parents, children, cross-system links |
| "Self-organization through rules" | PRQL queries + automation rules |
| "AI requires relational data" | Unified local cache enables Watcher/Integrator/Guide to reason across systems |

**Holon's unique differentiator**: FSP assumes a single knowledge graph you control. Holon's hybrid CRDT + Shadow Layer architecture treats **external server-authoritative systems** as first-class citizens with offline-first conflict resolution.

### Formalize Relationship Types

FSP defines four relationship roles. Holon could explicitly model these:

```rust
pub enum RelationshipRole {
    BelongsTo,    // ↑ Structural parent (already: parent_id, project membership)
    ComesFrom,    // ← Provenance (e.g., "this task was created from this email")
    LeadsTo,      // → Purpose (e.g., "this task advances this project goal")
    Contextual,   // ○ Association (existing backlinks, tags, cross-system refs)
}
```

This would strengthen the Integrator's context surfacing and enable richer queries like "show everything that leads to this project goal."

### Four-Question Method for Entity Design

When defining new entity types, answer sequentially:

| Question | Maps To |
|----------|---------|
| 1. Where does it live? | `source()` + cache table destination |
| 2. What does it connect to? | Relationship trait definitions |
| 3. How should it present? | RenderSpec / PRQL render clause |
| 4. What can it do? | `#[operations_trait]` implementations |

### Track "Functional Debt"

FSP introduces useful terminology:
> "Friction signals missing function... indicates undefined family rules, missing relationships, inadequate projections"

The Watcher could track **functional debt** — places where repetitive manual work suggests missing automation:

| Friction Type | Design Deficiency | Potential Fix |
|---------------|-------------------|---------------|
| Repetitive linking | Missing relationship rules | Auto-link via Integrator |
| Manual status updates | No propagation rules | Bi-directional sync |
| Search archaeology | Poor presentation/indexing | Unified semantic search |
| Orphaned items | Undefined destination | Inbox for unprocessed items |
| Context switching | Inadequate projections | Context Bundles in Flow mode |

### Two Presentation Modes

FSP distinguishes "Direct View" (canonical display) from "Contextual Projection" (adapted display in different contexts). RenderSpec could explicitly support both:

```prql
render direct(full_detail_template(...))
render projected(compact_row_template(...))
```

This aligns with existing vision (Orient mode = dense info, Flow mode = focused task) but could be more explicitly modeled.

### Scale Advantage Framing

FSP articulates why unified architecture matters:
> "In functional systems, value per entity increases with scale" — relationship density grows, patterns become visible, automation possibilities multiply.

This is the core value proposition of Holon's unified data layer: the opposite of traditional systems where findability and manageability degrade with growth. The mechanism is topological, not volumetric — each confirmed edge between existing entities activates traversal paths between all previously connected nodes. The graph grows smarter without growing larger. Intelligence in a relational system is a property of edge density, not node count.

---

## Related Documents

- [../Vision.md](../Vision.md) — Technical architecture and implementation details
- [AI.md](AI.md) — AI feature specifications and development path
- [PetriNet.md](PetriNet.md) — Petri-Net foundations, Digital Twins, WSJF sorting
- [Strategy/NameIdeas.md](../Strategy/NameIdeas.md) — Naming exploration (historical)

---

## The Holon Promise

When you use Holon, you will:

1. **Trust** that nothing important is forgotten
2. **Know** that what you're working on is the right thing
3. **Access** any information you need instantly
4. **Achieve** flow states more frequently
5. **Grow** through insights about your patterns
6. **Integrate** your whole life — professional and personal — into a coherent whole

You are a holon. Holon helps you see that clearly.
