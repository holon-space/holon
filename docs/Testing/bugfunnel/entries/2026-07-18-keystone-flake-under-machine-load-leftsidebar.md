---
id: 2026-07-18-keystone-flake-under-machine-load-leftsidebar
date: 2026-07-18
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Keystone flake under machine load: `SutFocusWrite::apply_navigate_focus`
  LeftSidebar navigation-intent settle exceeded its 5s CDC deadline
  (`await_sidebar_nav_intent`,
  `crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:891`)
  during a heavily-contended run; rerun green — a load-sensitivity /
  oracle-margin issue, not a prod defect
source_line: 807
---

## Bug

Keystone flake under machine load: `SutFocusWrite::apply_navigate_focus`
LeftSidebar navigation-intent settle exceeded its 5s CDC deadline
(`await_sidebar_nav_intent`,
`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:891`)
during a heavily-contended run; rerun green — a load-sensitivity /
oracle-margin issue, not a prod defect

## Missing piece

the 5s `soak_deadline` for the sidebar nav-intent to stream is a fixed
wall-clock margin that a load-starved runner can blow even when the system
is correct; needs a load-normalized deadline (or a CDC-quiescence gate
instead of a wall-clock deadline) so the oracle does not false-red under
contention

## Remedy

OPEN — flaked once 2026-07-18 under machine load, rerun green; margin/oracle
tuning, not a prod defect
