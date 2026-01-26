---
name: holon-architecture-tests
description: Architecture rule enforcement tests — verifies crate dependency structure and layering rules
type: reference
source_type: component
source_id: crates/holon-architecture-tests/
category: service
fetch_timestamp: 2026-04-23
---

## holon-architecture-tests (crates/holon-architecture-tests)

**Purpose**: Enforces architectural invariants via tests. Verifies that crate dependency rules are followed (e.g., no crate below `holon-api` imports from crates above it), preventing accidental coupling.

### Related

- **deny.toml**: cargo-deny config for license and dependency auditing
- **holon-api**: lowest-level shared crate — all others may depend on it
- **holon**: root backend — must not be imported by frontends directly (they use traits)
