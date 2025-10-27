# Cucumber Integration Tests - Handoff Document

## Overview

This document provides context for the Cucumber integration test redesign. The implementation is **COMPLETE AND FULLY TESTED** with all 4 realistic end-to-end user scenarios passing. Tests verify the full stack: file watcher → database sync → CDC streaming → real-time UI updates.

## Implementation Status: ✅ COMPLETE AND TESTED

All core infrastructure is implemented and all tests passing:
- ✅ WidgetStateModel with WidgetLocator system
- ✅ CDC stream helpers (drain_stream, wait_for_text_in_widget)
- ✅ TestContext with initial_widget_with_stream()
- ✅ HolonWorld with widget state tracking
- ✅ Comprehensive step definitions for workflow testing
- ✅ app_workflow.feature with realistic scenarios
- ✅ Grandchildren virtual table re-enabled (recursive CTEs now supported)
- ✅ All 4 scenarios and 22 steps passing

**Recent Fixes (2026-01-02):**
- ✅ Fixed type coercion bug in turso.rs (numeric strings like "20251231" preserved as strings)
- ✅ Fixed widget text extraction to respect render templates instead of hardcoding "content" field
- ✅ Increased file watcher initialization delay to 3000ms for reliable test execution
- ✅ All integration tests verified and passing

## Test Results

**All tests passing:** `cargo test --test cucumber`

```
1 feature
4 scenarios (4 passed)
22 steps (22 passed)
```

**Scenarios verified:**
1. ✅ Initial app load with layout file - Widget structure and content rendering
2. ✅ Journal content appears in widget - File sync to database to UI
3. ✅ Backend operation triggers CDC update - Real-time UI updates via CDC streaming
4. ✅ Editing org file triggers widget update - File watcher → database → CDC → UI

## Current State

### What Works
- **File sync**: Writing org files triggers automatic sync to Loro → EventBus → CacheEventSubscriber → blocks table
- **`initial_widget`**: Returns the root layout widget with query results populated and CDC stream for real-time updates
- **`from children` virtual table**: Queries children of a context block correctly
- **`from roots` virtual table**: Queries blocks with `parent_id IS NULL`
- **`from siblings` virtual table**: Queries siblings of the current block
- **`from grandchildren` virtual table**: ✅ NOW ENABLED - Uses JOIN with children CTE (recursive CTEs now supported in Turso MatViews)
- **Block operations**: `execute_operation("blocks", "create/update/delete", params)` work without requiring `doc_id`
- **CDC streaming**: WidgetStateModel tracks UI state by applying CDC events, enabling real-time UI assertions

### Known Limitations
- **PRQL queries require `render()`**: All PRQL queries must include a `render()` call because `compile_query` uses the widget rendering pipeline.
- **`content_type` values**: Org headings have `content_type = "text"`, not "heading". Source blocks have `content_type = "source"`.
- **File watcher initialization**: Tests require a 3-second delay after backend initialization to ensure the file watcher is fully ready before file operations.

### Key Files
- `crates/holon-integration-tests/tests/cucumber.rs` - Step definitions
- `crates/holon-integration-tests/tests/features/backend_operations.feature` - Feature file
- `crates/holon-integration-tests/src/test_context.rs` - TestContext wrapper
- `crates/holon-integration-tests/src/polling.rs` - Async polling utilities
- `crates/holon/src/api/backend_engine.rs` - PRQL stdlib and query execution

## New Test Direction

### Goal
Tests should simulate realistic user workflows:
1. App starts with an `index.org` layout file
2. User sees sidebar with recent pages, main area with journal
3. User performs operations (create blocks, edit content)
4. Changes propagate to UI via CDC notifications
5. File modifications trigger UI updates

### Proposed Test Structure

```gherkin
Feature: Holon App Workflow
  As a Holon user
  I want to see my knowledge base rendered as widgets
  And have changes propagate in real-time

  Background:
    Given the following "index.org" layout:
      """
      * Layout
      :PROPERTIES:
      :ID: app-layout
      :END:
      #+BEGIN_SRC holon_prql
      from children
      filter content_type == "text"
      render (colums (render_entity this))
      #+END_SRC
      ** Recent
      :PROPERTIES:
      :ID: sidebar
      :WIDTH: 250
      :END:
      #+BEGIN_SRC holon_prql
      from blocks
      sort {-updated_at}
      take 10
      render (list item_template:(text this.content))
      #+END_SRC
      ** Main View
      :PROPERTIES:
      :ID: main-view
      :WIDTH: auto
      :END:
      #+BEGIN_SRC holon_prql
      from blocks
      join cf = current_focus (cf.block_id == blocks.id)
      render (list item_template:(text this.content))
      #+END_SRC
      """
    And the following "journals/20251231.org" journal:
      """
      * Morning Thoughts
      :PROPERTIES:
      :ID: morning-thoughts
      :END:
      Starting the day with planning.
      """

  Scenario: Initial app load shows layout with sidebar and journal
    When I open the app # This triggers initial_widget call
    Then I should see 2 colums
    And column 1 should show
      """
      Recent
        Morning Thoughts
      """
    And column 2 should show
      """
      20251231
      * Morning Thoughts
      Starting the day with planning.
      """

  Scenario: Creating a block updates the sidebar
    When I open the app # This triggers initial_widget call
    And I execute operation "blocks.create" with:
      | id        | new-block-1        |
      | parent_id | holon-doc://__root_doc__ |
      | content   | New idea           |
    Then within 5 seconds column 1 should contain "New idea"

  Scenario: Editing org file updates the UI
    When I open the app # This triggers initial_widget call
    And I append to "journals/20251231.org":
      """
      * Afternoon Notes
      :PROPERTIES:
      :ID: afternoon-notes
      :END:
      Meeting went well.
      """
    Then within 5 seconds column 2 should contain "Afternoon Notes"
```

The `column N should contain` would of course not render anything, but keep track of the initial data + the changes received via CDC notifications, build up state of what would currently be displayed (including which fields are used in the widgets) and compare that.
Pure text matching is sufficient for these tests.
We can later extend this to e.g. tree views which in Gherkin could be written as:

```gherking
Then column 2 should contain:
  """
  20251231
  * Morning Thoughts
    Starting the day with planning.
    * More details on planning
  * Afternoon Notes
    Meeting went well.
  """
```

## Technical Details for Implementation

### TestContext Methods Available

```rust
// Create/write org files
ctx.create_document("filename.org") -> doc_uri
ctx.write_org_file("filename.org", content) -> PathBuf

// Execute operations (no doc_id needed)
ctx.execute_operation("blocks", "create", params)
ctx.execute_operation("blocks", "update", params)
ctx.execute_operation("blocks", "delete", params)

// Get initial widget
ctx.initial_widget() -> WidgetSpec

// Query with PRQL (requires render() call)
ctx.query("from blocks | ... | render (list ...)") -> Vec<HashMap<String, Value>>

// Access engine for query_and_watch
ctx.engine().query_and_watch(prql, params, context) -> (WidgetSpec, RowChangeStream)
```

### Block Operation Parameters

```rust
// Create block
params.insert("id", Value::String(block_id));
params.insert("parent_id", Value::String(parent_id)); // Can be doc_uri or block_id
params.insert("content", Value::String(content));
params.insert("content_type", Value::String("text")); // or "source"

// For source blocks, also include:
params.insert("source_language", Value::String("holon_prql"));

// Update block
params.insert("id", Value::String(block_id));
params.insert("field", Value::String("content"));
params.insert("value", Value::String(new_value));

// Delete block
params.insert("id", Value::String(block_id));
```

### PRQL Stdlib Virtual Tables

```prql
-- Available in all queries (injected automatically)
let children = (from blocks | filter parent_id == $context_id)
let roots = (from blocks | filter parent_id == null)
let siblings = (from blocks | filter parent_id == $context_parent_id)

-- $context_id is set via QueryContext::for_block(block_id, parent_id)
-- When no context provided, $context_id is NULL (children returns empty)
```

### Watching for Changes

```rust
// Set up CDC watch
let context = QueryContext::for_block(block_id, None);
let (widget_spec, mut change_stream) = engine
    .query_and_watch(prql, params, Some(context))
    .await?;

// Process initial data
process_widget(widget_spec);

// Listen for changes
while let Some(change) = change_stream.next().await {
    match change {
        RowChange::Insert(row) => { ... }
        RowChange::Update { old, new } => { ... }
        RowChange::Delete(row) => { ... }
    }
}
```

### Document URIs
- Format: `holon-doc://filename.org`
- Root document: `holon-doc://__root_doc__` (constant `ROOT_DOC_ID`)
- Blocks have `doc_id` field pointing to their document

### Block ID Conventions
- Headings: Use `:ID:` property value, e.g., `my-heading`
- Source blocks: `{parent_id}::src::{index}`, e.g., `my-heading::src::0`

## Implementation Tasks

1. **Create realistic test fixtures**
   - Standard `index.org` with sidebar/main layout
   - Sample journal file structure
   - Helper to set up common scenarios

2. **Add new step definitions**
   - `Given the following {string} layout/journal` - Write org file with content
   - `When I call initial_widget` - Already exists
   - `Then the widget should contain {layout_type}` - Inspect WidgetSpec structure
   - `Then within {int} seconds the {area} should contain {string}` - Poll query results

3. **Add CDC testing helpers**
   - Set up `query_and_watch` with proper context
   - Collect changes over time window
   - Assert on received change notifications

4. **Remove obsolete tests**
   - Low-level CRUD scenarios that don't reflect user workflows
   - Tests that bypass the widget rendering pipeline

## Files to Modify

- `tests/features/backend_operations.feature` - Replace with new scenarios
- `tests/cucumber.rs` - Add new step definitions, remove obsolete ones
- `src/test_context.rs` - Add helpers for CDC testing if needed
- `src/polling.rs` - May need widget-aware polling helpers

## Questions to Resolve

1. How should journal pages be discovered? By doc_id pattern or explicit list?
2. Should tests verify the actual RenderSpec structure or just data?
3. How to test operations with undo/redo?
4. Should we test multi-document scenarios (links between pages)?

## Reference: WidgetSpec Structure

```rust
pub struct WidgetSpec {
    pub render_spec: RenderSpec,  // Widget tree structure
    pub data: Vec<HashMap<String, Value>>,  // Query results
    pub row_templates: Vec<RowTemplate>,  // Per-row rendering
    pub actions: Vec<ActionSpec>,  // Available operations
}
```

The `render_spec` contains the widget tree defined by the `render()` call in PRQL.
The `data` contains the query result rows that populate the widgets.
# Cucumber Integration Tests Redesign

## Goal
Transform low-level CRUD Cucumber tests into realistic end-to-end user workflow tests with CDC streaming for real-time UI state tracking.

## Files to Modify

| File | Changes |
|------|---------|
| `crates/holon-integration-tests/Cargo.toml` | Add `indexmap` dependency |
| `crates/holon-integration-tests/src/widget_state.rs` | **NEW** - WidgetStateModel + WidgetLocator |
| `crates/holon-integration-tests/src/lib.rs` | Export new widget_state module |
| `crates/holon-integration-tests/src/test_context.rs` | Add `initial_widget_with_stream()` |
| `crates/holon-integration-tests/src/polling.rs` | Add CDC stream drain/wait helpers |
| `crates/holon-integration-tests/tests/cucumber.rs` | New step definitions + updated HolonWorld |
| `crates/holon-integration-tests/tests/features/app_workflow.feature` | **NEW** - Realistic workflow scenarios |
| `crates/holon-integration-tests/tests/features/backend_operations.feature` | Keep 1 CRUD scenario, remove rest |

## Implementation Steps

### Step 1: Create WidgetStateModel
Create `src/widget_state.rs` with:
```rust
pub struct WidgetStateModel {
    rows: IndexMap<String, HashMap<String, Value>>,  // Preserves insertion order
    render_spec: RenderSpec,
}

/// Widget locator for targeting specific widgets in assertions
/// Designed for extensibility - "column 1" is just one locator type
pub enum WidgetLocator {
    Column(usize),           // "column 1", "column 2"
    ViewId(String),          // "sidebar", "main-view"
    All,                     // Match all widgets
    // Future: Path(Vec<String>) for "main-view > list > item 3"
}

impl WidgetLocator {
    /// Parse from Gherkin step string
    pub fn parse(s: &str) -> Self {
        if s.starts_with("column ") {
            let n = s.strip_prefix("column ").unwrap().parse().unwrap_or(0);
            WidgetLocator::Column(n)
        } else {
            WidgetLocator::ViewId(s.to_string())
        }
    }
}
```

Key methods:
- `from_widget_spec(spec)` - Initialize from WidgetSpec
- `apply_change(change: &RowChange)` - Apply CDC events
- `extract_text(locator: &WidgetLocator)` - Get text for widget(s) matching locator
- `contains_text(locator: &WidgetLocator, expected: &str)` - Check if widget contains text

### Step 2: Update TestContext
Add to `src/test_context.rs`:
```rust
pub async fn initial_widget_with_stream(&self) -> Result<(WidgetSpec, RowChangeStream)> {
    self.ctx.engine().initial_widget().await
}
```

### Step 3: Add Stream Helpers
Add to `src/polling.rs`:
- `drain_stream(stream)` - Non-blocking drain of pending events
- `wait_for_text_in_column(stream, state, column, text, timeout)` - Poll until text appears

### Step 4: Update HolonWorld
Add fields to `HolonWorld` struct:
```rust
widget_spec: Option<WidgetSpec>,
widget_state: Option<WidgetStateModel>,
change_stream: Option<RowChangeStream>,
```

### Step 5: Add New Step Definitions

**Given steps:**
- `the following {string} layout:` - Write org file with docstring
- `the following {string} journal:` - Write journal file (creates parent dirs)

**When steps:**
- `I open the app` - Call initial_widget_with_stream, init WidgetStateModel
- `I execute operation {string} with:` - Execute operation with table params
- `I append to {string}:` - Append docstring to existing file

**Then steps (using generic widget locator):**
- `I should see {int} columns` - Assert view count in RenderSpec
- `the {string} widget should show` - Exact text match (docstring), locator parsed from string
- `the {string} widget should contain {string}` - Substring match
- `within {int} seconds the {string} widget should contain {string}` - CDC polling assertion

The `{string}` locator supports: `"column 1"`, `"column 2"`, `"sidebar"`, `"main-view"`, etc.
This allows future expansion to more precise targeting like `"main-view > list > item 3"`.

### Step 6: Create New Feature File
Create `tests/features/app_workflow.feature` with scenarios:
1. Initial app load shows layout structure
2. Journal content appears in UI
3. Creating a block updates UI via CDC
4. Editing org file triggers UI update

### Step 7: Clean Up Old Tests
Reduce `backend_operations.feature` to 1-2 representative CRUD scenarios for regression.

## Key Design Decisions

### Widget Locator System
`WidgetLocator` enum abstracts widget targeting for assertions:
- `Column(n)` - "column 1", "column 2" → nth view in iteration order
- `ViewId(s)` - "sidebar", "main-view" → match by view ID
- `All` - match all widgets (for global assertions)

Resolution: Map locator to `ViewSpec` filter, then filter `rows` by that filter.
Fallback for `Column(n)` out of bounds: return all rows.

### Text Extraction
Extract `content` field from rows. Future: add hierarchy support for indented tree views.

### Stream Lifecycle
- `HolonWorld` owns the stream
- Drain pending events before each assertion
- Apply all changes to `WidgetStateModel` before checking

## Testing Order
1. Run new tests first to verify infrastructure
2. Keep old tests during transition
3. Remove old tests once new tests are stable
