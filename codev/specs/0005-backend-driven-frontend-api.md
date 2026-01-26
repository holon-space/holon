# Specification: Backend-Driven Frontend API

## Metadata
- **ID**: spec-2025-12-26-backend-driven-frontend
- **Status**: draft
- **Created**: 2025-12-26
- **Updated**: 2025-12-26

## Clarifying Questions Asked
- User expressed desire to make frontends thinner by moving more logic to backend
- User wants backend to drive what the UI displays while keeping frontend-specific optimizations (lazy rendering, animations)
- User wants better E2E testability without requiring actual UI
- Navigation should work via IVM (Incremental View Maintenance) - changing navigation state triggers automatic UI updates
- Navigation should not be part of undo/redo stack
- Allow navigating to any block (not just documents) for zoom-in functionality
- Return partial results when one region fails (keep app working)

## Problem Statement

The Holon application has a single backend with multiple frontends (Flutter, TUI, MCP, WaterUI). Currently, significant logic is duplicated across frontends:

1. **Query Selection**: Each frontend hardcodes PRQL queries to determine what to display
2. **Navigation State**: Frontends independently track which document is being viewed
3. **Layout Decisions**: Frontends parse REGION properties to place blocks in sidebars
4. **View Type Interpretation**: Frontends decide how to render blocks based on VIEW property

This duplication:
- Increases maintenance burden when adding new frontends
- Makes E2E testing difficult (tests need real UI to verify correct views are displayed)
- Can lead to inconsistent behavior across frontends
- Violates separation of concerns (frontends know too much about business logic)

## Current State

### Backend (crates/holon/)
- `BackendEngine` provides: `compile_query`, `query_and_watch`, `execute_operation`, `undo/redo`
- `RenderSpec`: AST of widgets (list, tree, row, etc.) with operations wired in
- CDC streaming via Turso materialized views for reactive updates
- Multi-view support exists but isn't used (`views: HashMap<String, ViewSpec>` in RenderSpec)

### Flutter Frontend (frontends/flutter/)
- `main.dart:525-528` has hardcoded PRQL query:
  ```dart
  const defaultMainQuery = r'''
  from blocks
  render (list sortkey:sort_key item_template:(render_entity this))
  ''';
  ```
- `navigation_provider.dart` manages document navigation state client-side
- `index_layout_provider.dart` parses blocks with REGION property to create layout:
  ```dart
  final prql = '''
    from blocks
    filter s"properties LIKE '%REGION%'"
    ...
  ''';
  ```
- `render_interpreter.dart` interprets RenderSpec to Flutter widgets
- Hierarchical sorting done client-side (Turso materialized views don't support ORDER BY)

### TUI Frontend (frontends/tui/)
- Similar pattern: hardcoded queries, client-side navigation, render interpretation

### MCP Frontend (frontends/mcp/)
- Thin wrapper around BackendEngine (good reference for minimal frontend)
- Exposes raw API: `execute_prql`, `execute_operation`, `watch_query`

### E2E Tests (crates/holon-integration-tests/)
- `general_e2e_pbt.rs` tests mutations and CDC correctness
- Cannot currently test "clicking document X shows blocks of X" without UI

### Specific Frontend Logic That Should Move to Backend

| Logic | Current Location | Problem |
|-------|-----------------|---------|
| Initial query selection | `main.dart:525-528` | Hardcoded, duplicated |
| Navigation state | `navigation_provider.dart` | Frontend-specific, untestable |
| Layout region parsing | `index_layout_provider.dart` | Client-side parsing |
| View type interpretation | `index_layout_provider.dart:124-143` | Business logic in frontend |
| Document click handling | Frontend navigation provider | Duplicated, untestable |

## Desired State

### Core Architecture: Navigation via IVM

Navigation becomes **just another data mutation**. The same CDC/IVM mechanism that updates block content also updates what's displayed in each region.

```
User clicks block to zoom in
    ↓
Operation: navigation.focus(region="main", block_id="xxx")
    ↓
INSERT INTO navigation_history + UPDATE navigation_cursor
    ↓
Turso IVM propagates change through JOINed queries
    ↓
CDC emits changes for all affected views
    ↓
UI updates automatically
```

### Database Schema

```sql
-- Navigation history for back/forward (persists across restarts)
CREATE TABLE navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,           -- "main", "left_sidebar", etc.
    block_id TEXT,                  -- Block being viewed (NULL = home/root)
    timestamp TEXT DEFAULT (datetime('now'))
);

-- Current position in history per region
CREATE TABLE navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- Convenience view: current focus per region
-- This is what queries JOIN against
CREATE VIEW current_focus AS
SELECT
    nc.region,
    nh.block_id,
    nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;
```

### PRQL Queries with Navigation JOIN

Region queries JOIN against `current_focus` to display content based on navigation state:

```prql
# Main region: show children of the currently focused block
# Uses recursive CTE to get all descendants for tree view
from blocks
join side:left cf = (from current_focus | filter region == "main")
filter (
    # If focused on a block, show its descendants
    (cf.block_id != null && s"blocks.id IN (
        WITH RECURSIVE descendants AS (
            SELECT id FROM blocks WHERE parent_id = {cf.block_id}
            UNION ALL
            SELECT b.id FROM blocks b
            JOIN descendants d ON b.parent_id = d.id
        )
        SELECT id FROM descendants
        UNION SELECT {cf.block_id}
    )")
    # If at home (no focus), show root blocks
    || (cf.block_id == null && blocks.parent_id == "null")
)
render (tree
    hierarchical_sort:[parent_id, sort_key]
    item_template:(render_entity this)
)
```

When user navigates:
1. `navigation_cursor` points to new `navigation_history` entry
2. IVM recomputes the JOIN - different blocks now match
3. CDC streams the difference (old blocks removed, new blocks added)
4. UI updates with new content

### NavigationOperations

```rust
#[operations_trait(entity_name = "navigation", entity_short_name = "nav")]
pub trait NavigationOperations {
    /// Focus on a block (zoom in) - can be a document or any block
    #[operation(
        display_name = "Focus",
        description = "Navigate to view a block and its children"
    )]
    async fn focus(
        &self,
        region: String,    // "main", "left_sidebar"
        block_id: String,  // ID of block to focus on
    ) -> Result<()>;

    /// Go back in navigation history
    #[operation(
        display_name = "Go Back",
        description = "Navigate to previous view"
    )]
    async fn go_back(&self, region: String) -> Result<()>;

    /// Go forward in navigation history
    #[operation(
        display_name = "Go Forward",
        description = "Navigate to next view in history"
    )]
    async fn go_forward(&self, region: String) -> Result<()>;

    /// Go to home (unfocus - show root level)
    #[operation(
        display_name = "Go Home",
        description = "Return to root view"
    )]
    async fn go_home(&self, region: String) -> Result<()>;
}
```

### Navigation Breadcrumb Query

For displaying the current path (uses recursive CTE to get ancestors):

```prql
# Get ancestor chain from focused block to root
from blocks b
join cf = current_focus (cf.region == "main" && cf.block_id != null)
filter s"b.id IN (
    WITH RECURSIVE ancestors AS (
        SELECT id, parent_id, content FROM blocks WHERE id = {cf.block_id}
        UNION ALL
        SELECT p.id, p.parent_id, p.content FROM blocks p
        JOIN ancestors a ON p.id = a.parent_id
    )
    SELECT id FROM ancestors
)"
render (breadcrumb item_template:(text content:this.content))
```

### AppFrame: Layout Definition

AppFrame describes the **base layout** established at startup. Content changes flow through IVM/CDC:

```rust
/// Complete description of UI layout (established once at startup)
pub struct AppFrame {
    /// Named regions with their query configurations
    pub regions: HashMap<String, RegionHandle>,

    /// Global actions (sync, settings, etc.)
    pub global_actions: Vec<ActionSpec>,
}

pub struct RegionHandle {
    /// Unique identifier for this region
    pub id: String,

    /// The compiled PRQL query (includes current_focus JOIN)
    pub prql: String,

    /// Compiled render spec
    pub render_spec: RenderSpec,

    /// Initial data from query execution
    pub initial_data: Vec<HashMap<String, Value>>,

    /// CDC stream - updates automatically when navigation changes
    pub change_stream: RowChangeStream,

    /// Error if this region failed to initialize (partial failure support)
    pub error: Option<String>,
}

pub struct ActionSpec {
    pub id: String,
    pub display_name: String,
    pub icon: Option<String>,
    pub operation: OperationDescriptor,
}
```

### App Startup Flow

```rust
impl BackendEngine {
    /// Initialize app frame - called once at startup
    /// Returns long-lived layout with CDC streams that auto-update on navigation
    pub async fn init_app_frame(&self) -> Result<AppFrame> {
        // 1. Ensure navigation tables exist with defaults
        self.ensure_navigation_tables().await?;

        // 2. Load index document to get region configuration
        let index = self.load_index_document().await?;

        // 3. Build region queries (with current_focus JOINs)
        let mut regions = HashMap::new();

        for region_block in index.regions {
            let prql = self.build_region_query(&region_block)?;

            match self.query_and_watch(prql.clone(), HashMap::new()).await {
                Ok((render_spec, initial_data, stream)) => {
                    regions.insert(region_block.region.clone(), RegionHandle {
                        id: region_block.region,
                        prql,
                        render_spec,
                        initial_data,
                        change_stream: stream,
                        error: None,
                    });
                }
                Err(e) => {
                    // Partial failure - region has error but app continues
                    regions.insert(region_block.region.clone(), RegionHandle {
                        id: region_block.region,
                        prql,
                        render_spec: RenderSpec::error_placeholder(),
                        initial_data: vec![],
                        change_stream: RowChangeStream::empty(),
                        error: Some(e.to_string()),
                    });
                }
            }
        }

        Ok(AppFrame { regions, global_actions: self.get_global_actions() })
    }
}
```

## Index Document Structure

The index document (org-mode file) defines regions:

```org
#+TITLE: Home

* Favorites
:PROPERTIES:
:REGION: left_sidebar
:VIEW: query
:END:
#+begin_src holon_prql
from blocks
filter properties->>'FAVORITE' == 'true'
render (list item_template:(link_block this))
#+end_src

* Main Content
:PROPERTIES:
:REGION: main
:END:
# Empty - uses default query that joins current_focus

* Recent
:PROPERTIES:
:REGION: right_sidebar
:VIEW: query
:END:
#+begin_src holon_prql
from blocks
sort updated_at desc
take 10
render (list item_template:(link_block this))
#+end_src
```

Backend processing:
1. Parse index document blocks with REGION property
2. For each region:
   - If has PRQL source block -> use that query
   - If empty -> use default query (with `current_focus` JOIN for navigation)
3. All default queries get automatic `current_focus` JOIN

## Simplified Frontend Code

### Before (Flutter main.dart)

```dart
const defaultMainQuery = r'''
from blocks
render (list sortkey:sort_key item_template:(render_entity this))
''';

// Watch navigation state
final navigation = ref.watch(navigationProvider);

// Parse index layout
final indexLayout = ref.watch(indexLayoutProvider);

// Execute query
final queryResult = ref.watch(queryResultByPrqlProvider(defaultMainQuery));

// Complex logic to decide what to show based on navigation + layout
if (navigation.isViewingDocument) {
  // Show document
} else if (indexLayout.isNotEmpty) {
  // Show index layout
} else {
  // Show fallback
}
```

### After

```dart
class MyApp extends ConsumerWidget {
  @override
  Widget build(BuildContext context, WidgetRef ref) {
    // Get app frame once at startup
    final appFrameAsync = ref.watch(appFrameProvider);

    return appFrameAsync.when(
      data: (frame) => Scaffold(
        body: Row(
          children: [
            // Left sidebar
            if (frame.regions.containsKey('left_sidebar'))
              SizedBox(
                width: 280,
                child: RegionWidget(region: frame.regions['left_sidebar']!),
              ),

            // Main content
            Expanded(
              child: RegionWidget(region: frame.regions['main']!),
            ),

            // Right sidebar (if exists)
            if (frame.regions.containsKey('right_sidebar'))
              SizedBox(
                width: 280,
                child: RegionWidget(region: frame.regions['right_sidebar']!),
              ),
          ],
        ),
      ),
      loading: () => LoadingScreen(),
      error: (e, _) => ErrorScreen(error: e),
    );
  }
}

class RegionWidget extends StatelessWidget {
  final RegionHandle region;

  @override
  Widget build(BuildContext context) {
    if (region.error != null) {
      return ErrorPlaceholder(error: region.error!);
    }

    return ReactiveQueryWidget(
      renderSpec: region.renderSpec,
      initialData: region.initialData,
      changeStream: region.changeStream,
      onOperation: (entity, op, params) {
        // All operations including navigation go through same path
        backendService.executeOperation(entity, op, params);
      },
    );
  }
}
```

No more:
- `navigation_provider.dart`
- `index_layout_provider.dart`
- Hardcoded PRQL queries
- Frontend deciding what to show based on navigation state

## E2E Testing

```rust
#[tokio::test]
async fn test_navigation_via_ivm() {
    let engine = create_test_engine().await;

    // Setup: Create a document with child blocks
    engine.execute_operation("blocks", "create", params!{
        "id" => "doc-1",
        "content" => "Test Document",
        "parent_id" => "null"
    }).await?;
    engine.execute_operation("blocks", "create", params!{
        "id" => "block-1",
        "content" => "Child block",
        "parent_id" => "doc-1"
    }).await?;

    // Initialize app frame
    let frame = engine.init_app_frame().await?;
    let main_region = &frame.regions["main"];

    // Initially at home - should see root documents
    assert!(main_region.initial_data.iter().any(|r|
        r.get("id").and_then(|v| v.as_string()) == Some("doc-1")
    ));
    assert!(!main_region.initial_data.iter().any(|r|
        r.get("id").and_then(|v| v.as_string()) == Some("block-1")
    ));

    // Navigate to document (zoom in)
    engine.execute_operation("navigation", "focus", params!{
        "region" => "main",
        "block_id" => "doc-1"
    }).await?;

    // Wait for CDC (IVM propagation)
    let changes = collect_changes(&main_region.change_stream, Duration::from_millis(100)).await;

    // Should see block-1 appear (it's a child of doc-1)
    assert!(changes.iter().any(|c| matches!(c,
        ChangeData::Created { data, .. } if data.get("id") == Some(&Value::String("block-1".into()))
    )));

    // Go back
    engine.execute_operation("navigation", "go_back", params!{
        "region" => "main"
    }).await?;

    // Should see doc-1 again at root level
    let changes = collect_changes(&main_region.change_stream, Duration::from_millis(100)).await;
    assert!(changes.iter().any(|c| matches!(c,
        ChangeData::Created { data, .. } if data.get("id") == Some(&Value::String("doc-1".into()))
    )));
}
```

## Stakeholders
- **Primary Users**: Frontend developers (Flutter, TUI, future frontends)
- **Secondary Users**: E2E test authors, users of the application
- **Technical Team**: Backend developers, integration testers

## Success Criteria
- [ ] Navigation tables created (`navigation_history`, `navigation_cursor`, `current_focus` view)
- [ ] `NavigationOperations` trait implemented with `focus`, `go_back`, `go_forward`, `go_home`
- [ ] Recursive CTE works for fetching block descendants
- [ ] IVM correctly propagates navigation changes through JOINed queries
- [ ] `init_app_frame()` returns complete layout with region CDC streams
- [ ] Flutter frontend uses new API instead of hardcoded queries
- [ ] E2E tests can verify navigation behavior without UI
- [ ] No PRQL queries hardcoded in frontend code
- [ ] Navigation state persists across app restarts (stored in SQLite)
- [ ] All existing functionality preserved (no regressions)

## Constraints

### Technical Constraints
- Turso materialized views don't support ORDER BY (hierarchical sorting stays client-side)
- CDC streams are per-query; each region gets independent stream
- Flutter Rust Bridge requires careful type design for cross-language serialization
- Existing RenderSpec structure should be preserved (additive changes only)
- Recursive CTEs must work within Turso/libsql

### Business Constraints
- Must maintain backward compatibility with existing frontends during migration
- MCP frontend should continue to expose raw API for power users/AI tools

## Assumptions
- Index document format (REGION, VIEW properties) is stable
- All frontends can adopt the new API without breaking changes
- Navigation state should persist across app restarts
- Recursive CTEs perform acceptably for typical tree depths (<100 levels)

## Solution Approach

### Navigation as Data Mutation + IVM

Instead of navigation methods returning new AppFrames, navigation is an **operation** that mutates database tables. Turso's IVM automatically propagates changes through JOINed queries.

**Pros**:
- Uses existing CDC/IVM infrastructure - no new update mechanisms
- Navigation automatically persists (it's in the database)
- Same mental model for all state changes
- E2E tests can verify state by querying tables

**Cons**:
- Requires careful query design with JOINs
- Recursive CTEs add complexity
- Debugging requires understanding IVM propagation

## Open Questions

### Critical (Blocks Progress)
- [x] How should navigation work? -> Via IVM, operations mutate tables
- [x] Should navigation be in undo stack? -> No
- [x] Navigate to documents only or any block? -> Any block (zoom in)

### Important (Affects Design)
- [ ] How to handle very deep trees in recursive CTE? (performance limit?)
- [ ] Should breadcrumb query be a separate region or part of main?

### Nice-to-Know (Optimization)
- [ ] Can we limit recursive CTE depth for performance?
- [ ] Should we prefetch adjacent blocks for smoother navigation?

## Performance Requirements
- **Initial Frame Load**: <200ms (including query execution)
- **Navigation (IVM propagation)**: <100ms from mutation to CDC delivery
- **Recursive CTE**: <50ms for trees up to 1000 blocks
- **Memory**: Per-region CDC streams should not exceed 10MB buffer

## Security Considerations
- Navigation operations should respect document permissions (if added later)
- Backend state should be isolated per-session (if multi-user)

## Test Scenarios

### Functional Tests
1. **Home to Block**: Navigate from home to a block, verify main region shows its children
2. **Back Navigation**: Navigate to block, go back, verify return to previous state
3. **Deep Zoom**: Navigate block -> child -> grandchild, verify correct content at each level
4. **Multi-Region Independence**: Sidebar navigation doesn't affect main region
5. **Persistence**: Restart app, verify navigation state preserved

### Non-Functional Tests
1. **Performance**: Recursive CTE with 1000-block tree under 50ms
2. **Memory**: CDC streams don't leak on rapid navigation
3. **Resilience**: One region query fails, others still render

## Dependencies
- **holon-api**: New types (AppFrame, RegionHandle)
- **holon**: NavigationOperations, recursive CTE helpers, init_app_frame()
- **frontends/flutter**: Migration to new API
- **frontends/tui**: Migration to new API (optional, for validation)

## References
- `crates/holon-api/src/render_types.rs`: Existing RenderSpec structure
- `frontends/flutter/lib/main.dart`: Current hardcoded query
- `frontends/flutter/lib/providers/navigation_provider.dart`: Current navigation
- `frontends/flutter/lib/providers/index_layout_provider.dart`: Current layout parsing
- `frontends/SPECS.md`: CDC-reactive architecture documentation
- `crates/holon-integration-tests/tests/general_e2e_pbt.rs`: Current E2E test approach

## Risks and Mitigation

| Risk | Probability | Impact | Mitigation Strategy |
|------|------------|--------|-------------------|
| Recursive CTE performance | Medium | Medium | Limit depth, test with large trees |
| IVM not propagating correctly | Low | High | Thorough testing of JOIN + IVM combo |
| Breaking existing frontends | Medium | High | Additive API, keep old methods working |
| Complexity in query construction | Medium | Medium | Helper functions for common patterns |

## Implementation Phases

### Phase 1: Navigation Tables + Operations
1. Create `navigation_history`, `navigation_cursor` tables
2. Create `current_focus` view
3. Implement `NavigationOperations` trait with `focus`, `go_back`, `go_forward`, `go_home`
4. Register as operation provider (NOT in undo stack)
5. Add unit tests for navigation operations

### Phase 2: Recursive CTE Helpers
1. Implement helper function to generate descendant CTE SQL
2. Implement helper function to generate ancestor CTE SQL (for breadcrumb)
3. Test CTE performance with various tree sizes
4. Add PRQL integration for CTE usage

### Phase 3: Region Queries with Navigation JOIN
1. Create default region query template with `current_focus` JOIN
2. Test that changing navigation triggers IVM updates
3. Verify CDC streams deliver correct changes
4. Handle edge cases (empty focus, deleted blocks)

### Phase 4: init_app_frame() Implementation
1. Index document parsing (use existing code)
2. Build AppFrame from index regions with navigation JOINs
3. Handle partial failures gracefully
4. Add global actions

### Phase 5: Flutter Migration
1. Create `appFrameProvider` that calls `init_app_frame()` once
2. Create `RegionWidget` that renders any region
3. Replace hardcoded query with region from AppFrame
4. Remove `navigation_provider.dart`, `index_layout_provider.dart`
5. Verify no regressions

### Phase 6: E2E Navigation Tests
1. Add navigation state machine to PBT
2. Test transitions: home -> block -> child -> back -> different block
3. Verify IVM propagation correctness
4. Test persistence across "restarts"

## Notes

### Frontend Responsibilities After Migration

Frontends still handle:
- **Lazy rendering**: ListView.builder for large lists
- **Animations**: Smooth transitions between states
- **Native interactions**: Gestures, keyboard shortcuts, drag-drop
- **Hierarchical sorting**: Client-side (Turso materialized view limitation)
- **Scroll position**: Transient UI state
- **Selection state**: Which item is focused/selected

### Backward Compatibility

The existing `query_and_watch()` API remains available for:
- MCP tools (raw query access for AI)
- Power users building custom views
- Gradual migration of existing frontends

### Key Insight: Navigation = Data

Navigation is not special - it's just another piece of application state stored in the database. This means:
- Same CDC/IVM mechanism handles navigation updates
- Navigation naturally persists
- E2E tests can verify navigation by querying tables
- No special navigation API needed beyond operations
