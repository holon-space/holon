# Testing Quick Reference

## Run Tests

```bash
# BDD/Cucumber tests (backend logic)
cargo test --test cucumber

# Property-based tests
cargo test

# Watch mode (requires cargo-watch)
cargo watch -x 'test --test cucumber'
```

## Test Structure

```
Your Code              Test Type           Tool          Location
──────────────────────────────────────────────────────────────────
TaskStore logic    →   BDD/Integration    Cucumber      tests/features/
Storage backend    →   BDD/Integration    Cucumber      tests/features/
Loro sync          →   BDD/Integration    Cucumber      tests/features/
Data invariants    →   Property-based     PropTest      src/**/*_tests.rs
React components   →   Unit tests         Vitest        src/**/*.test.tsx
E2E flows          →   Manual testing     N/A           -
```

## Add New BDD Test

1. **Feature** (`tests/features/my_feature.feature`):
```gherkin
Feature: My Feature
  Scenario: Do something
    Given initial state
    When I perform action
    Then expected outcome
```

2. **Steps** (`tests/steps/my_steps.rs`):
```rust
#[when("I perform action")]
async fn perform_action(world: &mut AppWorld) {
    world.result = world.store.do_something();
}
```

3. **Export** (`tests/steps/mod.rs`):
```rust
mod my_steps;
pub use my_steps::*;
```

## Common Patterns

### Testing CRUD Operations
```gherkin
Scenario: Create item
  Given an empty store
  When I create an item with name "Test"
  Then the item should exist
  And it should have name "Test"
```

### Testing State Changes
```gherkin
Scenario: State transition
  Given item is in state "draft"
  When I publish the item
  Then item should be in state "published"
```

### Testing Error Handling
```gherkin
Scenario: Handle invalid input
  Given a valid document
  When I update with empty title
  Then I should see an error
```

## Debugging Failed Tests

```bash
# Run with backtrace
RUST_BACKTRACE=1 cargo test --test cucumber

# Run specific scenario
cargo test --test cucumber -- "Scenario name"

# Verbose output
cargo test --test cucumber -- --verbose
```

### PBT live inspection (panic pause + holon-live MCP)

When a property-based test panics, by default the embedded MCP server
tears down with the process. To hold the process open and attach the
`holon-live` MCP proxy to inspect live DB / Loro state:

```bash
PROPTEST_SEED=42 PBT_PAUSE_SECONDS=300 \
  cargo test -p holon-gpui --test gpui_ui_pbt --features pbt
```

`PBT_PAUSE_SECONDS=N` is the master switch — it installs a panic hook
that sleeps `N` seconds before the process tears down, and forces the
embedded MCP server up on port **8528** so `holon-live` can connect.

Full workflow (panic-hook details, step-level pauses, debugger setup):
[`PbtPauseAndLiveMcp.md`](PbtPauseAndLiveMcp.md).

## What to Test

### ✅ DO Test
- Business logic functions
- Data transformations
- State management
- Error conditions
- Edge cases

### ❌ DON'T Test
- Implementation details
- Private helper functions
- Third-party library behavior
- UI layout/styling

## Files Modified

- `Cargo.toml` - Added cucumber dependency
- `tests/cucumber.rs` - Test runner
- `tests/steps/` - Step definitions
- `tests/features/` - Gherkin scenarios

## Key Insight

**You're testing Tauri commands without the UI!**

```
Frontend (React) → invoke('add_task') → Tauri IPC → TaskStore.add_task()
                                                          ↑
Tests → TaskStore.add_task() ──────────────────────────────┘
        (Same logic, no IPC overhead)
```

Fast, reliable, works on macOS! 🎉
