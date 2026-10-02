---
id: 2026-10-02-advice-read-failure-clears-sidecar-at-debug
date: 2026-10-02
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  When an advice read fails, for any reason, the advice weaver clears the advice sidecar
  and logs only at DEBUG, so all advice disappears from the UI with no visible disclosure.
---

## Bug

A read-only architecture study (2026-10-02, D26.b census, item K8 "silent fallbacks") found
this site. Nothing ran; this comes from reading the code.

## Root cause

`recompute_sidecar` (`crates/holon-frontend/src/advice_weaver.rs:209-228`) runs the canonical
advice read. On `Err` it logs at `tracing::debug!` and calls `sidecar.clear()` (`:221-227`).
The rule-discovery read in `discover_active_rule` (`:299-312`) does the same: on a read error
it logs at DEBUG and returns `None`, and the caller then clears the sidecar (`:210-213`). The
doc comment justifies this with one transient case, a boot race in which the
`advice_rule_{slug}` matview does not exist yet. But the arm catches every error class,
including a malformed rule view or a SQL error that will never resolve. Then advice stays empty
for the whole session, and the only trace is a DEBUG line.

There is a related check that runs only in debug builds: `debug_assert!(active.len() <= 1)`
(`:338-342`). In a release build, two active rules do not fail; `active.pop()` keeps one of
them without a message.

Justified, not recorded as a defect: skipping an unparseable rule (`:328-333`). The rule block
shows its own status (ADR 0022).

## Missing piece

No test makes the advice read fail after boot, and no invariant states that a failed advice read
must become visible. The keystone sees an empty sidecar as a valid "no advice" state.

## Remedy

OPEN. Rung that closes the gap: a frontend test (the advice-weaver unit tier, with a
`QueryEngine` double) whose canonical read returns a non-"table not found" error. It asserts
that the failure is raised as a condition or a returned error, not cleared at DEBUG. It goes red
today. Fix direction: tolerate only the named boot-race error (matview not created yet), and
surface every other error. Change the `debug_assert!` on the active-rule count to an `assert!`
or a disclosed refusal.
