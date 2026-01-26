---
name: holon-layout-testing
description: GPUI layout and visual snapshot tests using insta and proptest
type: reference
source_type: component
source_id: crates/holon-layout-testing/
category: service
fetch_timestamp: 2026-04-23
---

## holon-layout-testing (crates/holon-layout-testing)

**Purpose**: GPUI-specific layout testing crate. Captures layout snapshots (insta) and runs property-based layout tests (proptest) to catch visual regressions.

### Testing Tools

- **insta**: snapshot testing — compares GPUI render output to stored snapshots
- **proptest**: generates random layout configurations to find rendering panics/invariant violations

### Known Issues Tracked

- `stable_cache_key` layout collision bug reproduced via proptest (discovered Apr 2026)
- BlockRef blank-panel: `size_full` vs `flex_1` in absolute-parent panels (fixed Apr 2026)

### Related

- **frontends/gpui**: the UI under test
- **holon-integration-tests**: complementary E2E tests (PBT, sync)
