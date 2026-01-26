---
name: holon-integration-tests
description: Property-based and integration tests — primary bug reproduction point for all UI and sync issues
type: reference
source_type: component
source_id: crates/holon-integration-tests/tests/
category: service
fetch_timestamp: 2026-04-23
---

## holon-integration-tests (crates/holon-integration-tests)

**Purpose**: End-to-end and property-based integration tests. The first stop for reproducing UI bugs before reaching for `eprintln`. Runs against a real Turso database — no mocks.

### Key Test Files

| File | Role |
|------|------|
| `tests/general_e2e_pbt.rs` | **Primary PBT** — covers all value functions (vfn1–vfn13+), invariants, and sync scenarios. Never add new PBT tests; extend this file only. |
| `tests/watch_ui.rs` | `watch_ui()` stream integration tests |
| `tests/bottom_dock.rs` | Bottom dock / mobile action bar tests |

### Invariants Tracked (general_e2e_pbt.rs)

| Invariant | Description |
|-----------|-------------|
| inv1–inv9 | Core block / sync invariants |
| inv10a | Display node structure exists |
| inv10b | WidgetSpec tree non-empty after render |
| inv10c | Error count hard assert |
| inv10d | Root widget matches RenderExpr |
| inv10e | Entity ID set is a subset of visible data |
| inv10f | Decompiled data matches query data |
| vfn11 | NavigateHome must clear focused_block globally |
| vfn12, vfn13 | ProviderCache wired through ValueFn::invoke |

### Architecture Notes

- Uses real Turso (SQLite) — not mocked
- `TestEnvironment` wraps `BackendEngine` + `TestServices`
- `TestServices::new_quiescent()` returns current_thread runtime; driver spawns queue but never runs, avoiding GPUI TestScheduler off-thread panic
- `vm_shared_collection` uses shadow interpret + ReactiveQueryResults with pre-populated rows + TreeItem wrapping
- `setup_watch()` takes a `language` parameter (prql/sql/gql)
- `tee` output to `/tmp/` before filtering — always

### RULES (from CLAUDE.md)

1. Whenever there's a UI bug, check if this PBT can reproduce it first
2. If it doesn't reproduce, make prod and E2E test more similar
3. **Do not add new PBT tests** — only use/extend the existing one

### Related

- **holon**: `BackendEngine` under test
- **holon-frontend**: `ReactiveViewModel` exercised via `TestServices`
- **frontends/gpui**: UI rendering paths tested here before GPUI launch
