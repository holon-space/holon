# ADR 0002: Trait-Based Unified Type System for Third-Party Integration

## Status

Proposed

## Context

Rusty Knowledge integrates data from heterogeneous third-party systems (Todoist, JIRA, Linear, Gmail, etc.) alongside native application data. Each system has fundamentally different data models:

- **Todoist**: Flat tasks with priority (1-4), sections, projects, labels
- **JIRA**: Issues with priority enum (Blocker/Critical/Major/Minor/Trivial), story points, sprints, epics, workflow states
- **Linear**: Issues with estimates, cycles, teams, projects, custom states
- **GitHub**: Issues with milestones, assignees, labels, projects v2

**The Challenge**: Create a unified abstraction that:

1. **Preserves system-specific richness**: JIRA sprints, story points, custom fields
2. **Enables common operations**: Mark complete, set priority, schedule due date
3. **Works with type-safe lens + predicate system**: No string-based field access
4. **Avoids forced mappings**: Todoist priority 1-4 ≠ JIRA priority enum
5. **Provides uniform UI**: Display tasks from all systems consistently
6. **Allows power-user access**: System-specific features available when needed

**Failed Approaches**:

### Approach 1: Single Unified Type (Rejected)

```rust
struct UnifiedTask {
    // Common fields
    id: String,
    title: String,
    priority: Option<Priority>,

    // Extension explosion (fatal flaw)
    jira_extensions: Option<JiraExtensions>,
    todoist_extensions: Option<TodoistExtensions>,
    linear_extensions: Option<LinearExtensions>,
}
```

**Fatal Flaws**:
- Every task carries optional fields for EVERY system
- No type safety: Can't enforce "JIRA tasks MUST have jira_extensions"
- Awkward lens generation for nested optionals
- Priority mapping is lossy and brittle
- Lies to type system: `priority: Option<Priority>` suggests it's optional everywhere

### Approach 2: Lowest Common Denominator (Rejected)

```rust
struct Task {
    id: String,
    title: String,
    completed: bool,  // Too simplistic - what about JIRA's In Progress state?
    // No priority - systems incompatible
    // No story points - Todoist doesn't have them
}
```

**Fatal Flaws**:
- Loses all system-specific richness
- Can't represent workflow states (JIRA: To Do, In Progress, Review, Done)
- Can't query for JIRA-specific features (story points > 5)
- Useless for power users

## Decision

We adopt a **three-layer trait-based architecture** that preserves native type richness while enabling UI uniformity through compositional capabilities and canonical projections.

### Architecture Overview

```
┌──────────────────────────────────────────────────────────┐
│                    UI LAYER                               │
│  Works with BlockView for uniform display                │
│  Can drill down to BlockContent for system-specific UI   │
└─────────────────────┬────────────────────────────────────┘
                      │
        ┌─────────────┴─────────────┐
        │                           │
┌───────▼────────┐      ┌──────────▼──────────┐
│  CAPABILITY    │      │   PROJECTION        │
│  TRAITS        │      │   LAYER             │
├────────────────┤      ├─────────────────────┤
│ Completable    │      │ BlockView (UI)      │
│ Prioritizable  │      │ BlockContent (data) │
│ Schedulable    │      │ Blocklike trait     │
│ Estimatable    │      │ BlockAdapter        │
│ Hierarchical   │      │                     │
└────────────────┘      └─────────────────────┘
        │                           │
        └─────────────┬─────────────┘
                      │
┌─────────────────────▼─────────────────────────────────────┐
│              NATIVE TYPE LAYER                             │
│  TodoistTask, JiraIssue, LinearIssue (system-specific)    │
│  Full richness, native field types, no forced mappings    │
│  QueryableCacheSource<Provider, NativeType>                     │
└────────────────────────────────────────────────────────────┘
```

### Layer 1: Native Types (Foundation)

Each integration defines its own rich type with system-native field types:

```rust
// Todoist uses native types
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "todoist_tasks")]
struct TodoistTask {
    #[primary_key] id: String,
    content: String,
    priority: i32,  // 1-4, native to Todoist
    due: Option<TodoistDue>,
    project_id: String,
    section_id: Option<String>,
    completed: bool,
    labels: Vec<String>,
}

// JIRA uses native types
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "jira_issues")]
struct JiraIssue {
    #[primary_key] key: String,
    summary: String,
    priority: JiraPriority,  // enum: Blocker, Critical, Major, Minor, Trivial
    due_date: Option<DateTime<Utc>>,
    story_points: Option<f32>,
    sprint: Option<SprintId>,
    epic: Option<EpicId>,
    status: JiraStatus,  // workflow-specific: ToDo, InProgress, InReview, Done, Closed
    assignee: Option<UserId>,
    custom_fields: HashMap<String, JsonValue>,
}
```

**Benefits**:
- ✅ No forced type conversions
- ✅ Full system richness preserved
- ✅ Entity macro generates type-safe lenses automatically
- ✅ Each type is independently versioned and evolved

### Layer 2: Capability Traits (Composition)

Traits define common capabilities with **associated types** to preserve native semantics:

```rust
// Core capability: anything that can be completed
trait Completable {
    fn is_completed(&self) -> bool;
    fn set_completed(&mut self, completed: bool) -> Result<()>;
}

// Core capability: anything with priority
trait Prioritizable {
    type PriorityType: Clone + Send + Sync + Display;

    fn priority(&self) -> Option<Self::PriorityType>;
    fn set_priority(&mut self, priority: Self::PriorityType) -> Result<()>;

    // For unified UI sorting/filtering
    fn normalized_priority(&self) -> NormalizedPriority;
}

// Core capability: anything with a due date
trait Schedulable {
    fn due_date(&self) -> Option<DateTime<Utc>>;
    fn set_due_date(&mut self, date: Option<DateTime<Utc>>) -> Result<()>;
}

// Optional capability: not all types implement
trait Estimatable {
    type EstimateType: Clone + Send + Sync;

    fn estimate(&self) -> Option<Self::EstimateType>;
    fn set_estimate(&mut self, estimate: Option<Self::EstimateType>) -> Result<()>;
}

// Optional capability: hierarchy support
trait Hierarchical {
    type ParentType: Clone + Send + Sync;

    fn parent_ref(&self) -> Option<Self::ParentType>;
    fn children_refs(&self) -> Vec<String>;
}

// Normalized enums for cross-system UI
enum NormalizedPriority {
    Critical,  // Todoist p1, JIRA Blocker/Critical
    High,      // Todoist p2, JIRA Major
    Medium,    // Todoist p3, JIRA Minor
    Low,       // Todoist p4, JIRA Trivial
}

enum CompletionState {
    NotStarted,
    InProgress,
    Completed,
}
```

**Implementation for each type**:

```rust
// Todoist implementations
impl Completable for TodoistTask {
    fn is_completed(&self) -> bool {
        self.completed
    }

    fn set_completed(&mut self, completed: bool) -> Result<()> {
        self.completed = completed;
        Ok(())
    }
}

impl Prioritizable for TodoistTask {
    type PriorityType = i32;  // Native Todoist priority (1-4)

    fn priority(&self) -> Option<i32> {
        Some(self.priority)
    }

    fn set_priority(&mut self, priority: i32) -> Result<()> {
        if !(1..=4).contains(&priority) {
            return Err("Todoist priority must be 1-4".into());
        }
        self.priority = priority;
        Ok(())
    }

    fn normalized_priority(&self) -> NormalizedPriority {
        match self.priority {
            1 => NormalizedPriority::Critical,  // p1 = most urgent
            2 => NormalizedPriority::High,
            3 => NormalizedPriority::Medium,
            4 => NormalizedPriority::Low,
            _ => NormalizedPriority::Medium,
        }
    }
}

// JIRA implementations
impl Prioritizable for JiraIssue {
    type PriorityType = JiraPriority;  // Native JIRA enum

    fn priority(&self) -> Option<JiraPriority> {
        Some(self.priority.clone())
    }

    fn set_priority(&mut self, priority: JiraPriority) -> Result<()> {
        self.priority = priority;
        Ok(())
    }

    fn normalized_priority(&self) -> NormalizedPriority {
        match self.priority {
            JiraPriority::Blocker | JiraPriority::Critical => NormalizedPriority::Critical,
            JiraPriority::Major => NormalizedPriority::High,
            JiraPriority::Minor => NormalizedPriority::Medium,
            JiraPriority::Trivial => NormalizedPriority::Low,
        }
    }
}

// TodoistTask does NOT implement Estimatable
// Compiler prevents using it in estimation contexts
impl Estimatable for JiraIssue {
    type EstimateType = f32;

    fn estimate(&self) -> Option<f32> {
        self.story_points
    }

    fn set_estimate(&mut self, estimate: Option<f32>) -> Result<()> {
        self.story_points = estimate;
        Ok(())
    }
}
```
<!--
Can I find out at runtime which traits a type implements?
Let's say I have a Box<dyn Any> that might be a TodoistTask or JiraIssue.
How could the UI layer check if it implements Completable or Estimatable?
-->

**Key Benefits**:
- ✅ Associated types preserve native semantics (no forced conversion)
- ✅ Type system enforces correctness (can't call `estimate()` on TodoistTask)
- ✅ Normalized view for UI without losing native precision
- ✅ One-way lossy mapping (Native → Normalized) for display only

### Layer 3: Projection Layer (UI)

#### 3a. BlockContent (Stored Data)

Following expert feedback, we rename the current `Block` enum to `BlockContent` to clarify it represents stored data:

```rust
// Enum representing all possible block content types
pub enum BlockContent {
    // Native types
    Todo(Todo),
    Heading(Heading),
    Divider(Divider),
    Embed(Embed),

    // External types
    TodoistTask(TodoistTask),
    JiraIssue(JiraIssue),
    LinearIssue(LinearIssue),
}

// Composition: Block = metadata + content
pub struct Block {
    pub data: BlockData,  // id, parent_id, created_at, etc.
    pub content: BlockContent,
}
```

<!--
Having an enum for BlockContent is rather unfortunate because we need to hardcode all possible types / external systems.
Are there other ways?
-->

**Benefits**:
- ✅ Sum type ensures exhaustive matching
- ✅ Compiler enforces handling all cases
- ✅ Clear distinction: BlockContent = stored, BlockView = rendered

#### 3b. BlockView (UI Projection)

Transient struct created on-the-fly for UI rendering:

```rust
// NOT stored in DB, created for UI display
#[derive(Debug, Clone)]
pub struct BlockView<'a> {
    pub id: BlockId,
    pub parent_id: Option<BlockId>,
    pub title: Cow<'a, str>,
    pub completion_state: CompletionState,
    pub priority_view: Option<NormalizedPriority>,
    pub due_date: Option<DateTime<Utc>>,
    pub estimate_view: Option<f32>,  // Normalized to story points
    pub has_children: bool,
    pub source: &'static str,  // "native", "todoist", "jira", "linear"
    pub source_id: String,  // Original ID for drill-down
    pub tags: Cow<'a, [String]>,
}

// Trait for projecting to view
trait Blocklike {
    fn to_block_view(&self) -> BlockView;
}

impl Blocklike for TodoistTask {
    fn to_block_view(&self) -> BlockView {
        BlockView {
            id: BlockId::from_external("todoist", &self.id),
            parent_id: self.section_id.as_ref()
                .map(|s| BlockId::from_external("todoist_section", s)),
            title: Cow::Borrowed(&self.content),
            completion_state: if self.completed {
                CompletionState::Completed
            } else {
                CompletionState::NotStarted
            },
            priority_view: Some(self.normalized_priority()),
            due_date: self.due.as_ref().and_then(|d| d.to_datetime()),
            estimate_view: None,  // Todoist doesn't have estimates
            has_children: false,
            source: "todoist",
            source_id: self.id.clone(),
            tags: Cow::Borrowed(&self.labels),
        }
    }
}

impl Blocklike for JiraIssue {
    fn to_block_view(&self) -> BlockView {
        BlockView {
            id: BlockId::from_external("jira", &self.key),
            parent_id: self.epic.as_ref()
                .map(|e| BlockId::from_external("jira_epic", e)),
            title: Cow::Borrowed(&self.summary),
            completion_state: match self.status {
                JiraStatus::Done | JiraStatus::Closed => CompletionState::Completed,
                JiraStatus::InProgress | JiraStatus::InReview => CompletionState::InProgress,
                _ => CompletionState::NotStarted,
            },
            priority_view: Some(self.normalized_priority()),
            due_date: self.due_date,
            estimate_view: self.story_points,
            has_children: false,  // Would query in adapter
            source: "jira",
            source_id: self.key.clone(),
            tags: Cow::Owned(vec![]),  // Would extract from labels/components
        }
    }
}
```

<!--
Related to my question above regarding runtime trait detection:
Can we have composable BlockViews that only access fields for the traits the native type implements?
E.g. one BlockView for Completable, another for Estimatable and the BlockView for JiraIssue includes both?
-->

#### 3c. BlockAdapter (Unified Queries)

Adapters make native types queryable as BlockView with best-effort predicate translation:

```rust
// Adapter: makes TodoistTask queryable as BlockView
struct TodoistBlockAdapter<C> {
    cache: C,  // QueryableCacheSource<TodoistProvider, TodoistTask>
}

impl<C> Queryable<BlockView> for TodoistBlockAdapter<C>
where
    C: Queryable<TodoistTask> + Send + Sync,
{
    async fn query(&self, predicate: Arc<dyn Predicate<BlockView>>) -> Result<Vec<BlockView>> {
        // Strategy 1: Try to translate BlockView predicate to TodoistTask predicate
        if let Some(translated) = self.try_translate_predicate(predicate.clone()) {
            // SQL-optimized query on native type
            let tasks = self.cache.query(translated).await?;
            return Ok(tasks.into_iter().map(|t| t.to_block_view()).collect());
        }

        // Strategy 2: Fallback to in-memory filtering
        let all_tasks = self.cache.query(Arc::new(AlwaysTrue)).await?;
        Ok(all_tasks.into_iter()
            .map(|t| t.to_block_view())
            .filter(|view| predicate.test(view))
            .collect())
    }
}

impl<C> TodoistBlockAdapter<C> {
    fn try_translate_predicate(&self, view_pred: Arc<dyn Predicate<BlockView>>)
        -> Option<Arc<dyn Predicate<TodoistTask>>>
    {
        // Simple translation for common fields
        // Phase 1: Return None (always use fallback)
        // Phase 2: Translate common predicates (title, completed)
        // Phase 3: Smart introspection
        None
    }
}
```

<!--
If possible, I would like to have only the following classes/traits/impl per external system:
- One per item-class (e.g. TodoistTask, TodoistProject) implementing capability traits
- One DataSource per item-class (e.g. TodoistTaskSource, TodoistProjectSource), see 0001-hybrid-sync-architecture.md
- One Provider per external system (e.g. TodoistProvider, JiraProvider)

The BlockAdapter instances should be generated from the structs (e.g. TodoistTask, JiraIssue).
The BlockAdapter::query could try to do a predicate-push-down for as many constraints as possible and
do an in-memory filter on the rest. I think that we should be able to map most predicates to SQL operations though.
-->

## Usage Patterns

### Pattern 1: System-Specific Queries (Optimal Performance)

```rust
use todoist_lenses::*;

// Query with native type → SQL optimization
let urgent_todoist = todoist_cache
    .query(Arc::new(Eq::new(PriorityLens, 1)))  // p1 in Todoist
    .await?;

// Full type richness available
for task in urgent_todoist {
    println!("{}: project={}, section={:?}",
        task.content, task.project_id, task.section_id);

    // Can access Todoist-specific fields
    for label in &task.labels {
        println!("  Label: {}", label);
    }
}
```

### Pattern 2: Unified Queries (Convenience)

```rust
use block_view_lenses::*;

// Query across all sources as BlockView
let all_in_progress = unified_query
    .query(Arc::new(Eq::new(CompletionStateLens, CompletionState::InProgress)))
    .await?;

// Uniform BlockView for UI
for view in all_in_progress {
    println!("{} [{}]", view.title, view.source);

    // Can drill down to native type if needed
    if view.source == "jira" {
        // Load full JiraIssue for system-specific UI
    }
}
```

### Pattern 3: Mixed (Best of Both Worlds)

```rust
// Query each system with native predicates (SQL optimized)
let todoist_critical = todoist_cache
    .query(Arc::new(Eq::new(TodoistPriorityLens, 1)))
    .await?;

let jira_blockers = jira_cache
    .query(Arc::new(Eq::new(JiraPriorityLens, JiraPriority::Blocker)))
    .await?;

// Convert to BlockView for uniform display
let all_critical: Vec<BlockView> = todoist_critical.into_iter()
    .map(|t| t.to_block_view())
    .chain(jira_blockers.into_iter().map(|j| j.to_block_view()))
    .collect();

// Display uniformly but preserve source identity
for view in all_critical {
    println!("[{}] {} (priority: {:?})",
        view.source, view.title, view.priority_view);
}
```

## Implementation Strategy

### Phase 1: Core Traits + Block Projection (Foundation)

**Goal**: Establish trait system and projection pattern

1. **Refactor existing Block**:
   - Rename `Block` enum → `BlockContent`
   - Create `struct Block { data: BlockData, content: BlockContent }`
   - Update all references

2. **Define capability traits**:
   - Create `src/core/capabilities.rs`
   - Implement `Completable`, `Prioritizable`, `Schedulable`
   - Implement for existing `Task` struct (proof of concept)

3. **Create BlockView**:
   - Define `struct BlockView<'a>`
   - Implement `Blocklike` trait
   - Implement for `Task` (validate pattern)

4. **Create first adapter**:
   - `TaskBlockAdapter` for internal tasks
   - Validate predicate pass-through vs fallback logic

**Validation**: Core patterns work with existing internal types before tackling external integrations

### Phase 2: First External Integration (Todoist)

**Goal**: Prove patterns work with real third-party system

1. **Complete TodoistTask type**:
   - Full field definitions
   - Implement all applicable capability traits
   - Implement Blocklike

2. **Create TodoistBlockAdapter**:
   - Simple predicate translation (completed, title only)
   - Fallback for complex predicates
   - Measure performance difference

3. **Test sync patterns**:
   - Native queries (SQL optimized)
   - BlockView queries (convenience)
   - Mixed queries

**Validation**: Trait system scales to real external system, performance acceptable

### Phase 3: Multiple Integrations (Generalization)

**Goal**: Validate patterns generalize across systems

1. **Add JIRA integration**:
   - JiraIssue with rich types (workflow states, story points)
   - Implement capability traits with associated types
   - Validate trait composition (not all traits for all types)

2. **Add Linear integration**:
   - LinearIssue with cycles, estimates
   - Test incompatible field handling

3. **Unified queries**:
   - Query across all three systems as BlockView
   - Validate cross-system sorting/filtering

**Validation**: No system-specific coupling, patterns are truly generic

### Phase 4: Advanced Features (Optimization)

**Goal**: Reduce boilerplate, improve performance

1. **Macro generation**:
   - Derive macro for capability traits
   - Auto-generate simple adapters
   - Reduce manual implementation burden

2. **Smarter predicate translation**:
   - Introspect predicate structure
   - Translate more complex predicates
   - Measure SQL optimization gains

3. **Incremental cache updates**:
   - Event-driven BlockView invalidation
   - Selective re-query on updates

## Consequences

### Positive

1. **Type Safety Preserved**
   - ✅ Native types use native field types (i32, enums, custom structs)
   - ✅ Compiler prevents invalid operations (can't estimate Todoist tasks)
   - ✅ Lenses work with concrete types, not `Value` wrappers
   - ✅ Associated types enforce correct priority scale per system

2. **SQL Optimization Where Possible**
   - ✅ Native queries compile to SQL directly
   - ✅ System-specific predicates fully optimized
   - ✅ Only cross-system incompatible queries fall back to in-memory

3. **UI Uniformity Without Loss**
   - ✅ BlockView provides consistent rendering interface
   - ✅ Can drill down to native type for system-specific UI
   - ✅ Semantic meaning preserved (CompletionState vs bool)

4. **Flexible Composition**
   - ✅ Not all traits apply to all types (compiler-enforced)
   - ✅ Can add new capabilities without touching existing types
   - ✅ Each integration evolves independently

5. **No Forced Mappings**
   - ✅ Todoist priority stays 1-4
   - ✅ JIRA priority stays enum
   - ✅ Normalized view only for display, not for operations

6. **Maintainability**
   - ✅ Clear separation of concerns (three layers)
   - ✅ Each layer testable independently
   - ✅ Macro generation reduces boilerplate (Phase 4)

### Negative

1. **Complexity**
   - ⚠️ Three-layer architecture has learning curve
   - ⚠️ Understanding trait bounds can be challenging
   - ⚠️ Adapter pattern adds indirection

2. **Performance Trade-offs**
   - ⚠️ Cross-system priority queries require in-memory filtering
   - ⚠️ BlockView creation has allocation overhead
   - ⚠️ Trait object dispatch has small runtime cost
   - **Mitigation**: Measure first, optimize if needed. Most queries are system-specific.

3. **Predicate Translation Limitations**
   - ⚠️ Not all BlockView predicates translate to native predicates
   - ⚠️ Complex predicates always fall back to in-memory
   - ⚠️ Translation logic is manual (Phase 1-3)
   - **Mitigation**: Start with simple translation, measure performance impact, iterate

4. **Boilerplate (Phase 1-3)**
   - ⚠️ Manual trait implementations for each type
   - ⚠️ Manual Blocklike implementations
   - ⚠️ Manual adapter creation
   - **Mitigation**: Derive macros in Phase 4 eliminate most boilerplate

5. **One-Way Mapping**
   - ⚠️ Can't convert BlockView back to native type (lossy)
   - ⚠️ Edit operations must load native type
   - **Mitigation**: This is by design. BlockView is read-only projection.

### Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| Trait complexity overwhelming | Comprehensive examples, gradual rollout |
| Performance regression on queries | Measure before/after, optimize hot paths |
| Too much boilerplate | Invest in derive macros (Phase 4) |
| Predicate translation too limited | Start simple (completed, title), expand based on usage |
| Cross-system queries too slow | Accept trade-off, provide system-specific alternatives |

## Alternatives Considered

### Alternative 1: Dynamic Dispatch Only

**Approach**: Use `dyn Trait` everywhere, no concrete types

**Rejected Because**:
- Loses compile-time type checking
- Can't use lenses effectively (need concrete types)
- Associated types don't work with trait objects
- Performance overhead on every field access

### Alternative 2: Code Generation for All Types

**Approach**: Generate unified Task struct at compile time from schemas

**Rejected Because**:
- Rust macros can't do this level of synthesis
- Would need external build tool (custom generator)
- Still doesn't solve priority mapping problem
- Adds build complexity

### Alternative 3: Block as Storage Type

**Approach**: Store normalized BlockView in database

**Rejected Because**:
- Lossy conversion means we can't round-trip to native API
- Can't reconstruct JIRA-specific fields from Block
- Would need separate tables for system-specific data anyway
- Violates "native types as source of truth" principle

## Integration with ADR 0001

This ADR builds on ADR 0001 (Hybrid Sync Architecture):

| ADR 0001 Concept | How ADR 0002 Extends |
|------------------|----------------------|
| Native types (TodoistTask, JiraIssue) | ✅ Adds capability traits (Completable, Prioritizable) |
| QueryableCacheSource wrapper | ✅ Wrapped by BlockAdapter for unified queries |
| Type-safe lenses | ✅ Work with both native types AND BlockView |
| Predicate system | ✅ Translated from BlockView to native where possible |
| DataSource<T> trait | ✅ Implemented by native types, abstracted by adapters |

**Synergy**: ADR 0001 provides the storage and sync foundation, ADR 0002 adds the semantic layer for cross-system operations.

## References

- [ADR 0001: Hybrid Sync Architecture](/docs/adr/0001-hybrid-sync-architecture.md)
- [Architecture Document](/docs/architecture.md) - Original hybrid storage model
- [Architecture v2](/docs/architecture2.md) - Type-safe generic data management
- [Rust Trait Objects](https://doc.rust-lang.org/book/ch17-02-trait-objects.html)
- [Associated Types in Rust](https://doc.rust-lang.org/book/ch19-03-advanced-traits.html)
- [Projection Pattern](https://martinfowler.com/eaaDev/Projection.html)

## Decision Makers

- Martin (Project Owner)
- Claude Code (AI Assistant)
- Gemini 2.5 Pro (Expert Consultation)

## Date

2025-11-02

## Supersedes

None

## Superseded By

None (current)
