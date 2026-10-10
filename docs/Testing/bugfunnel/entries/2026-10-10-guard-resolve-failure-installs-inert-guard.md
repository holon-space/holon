---
id: 2026-10-10-guard-resolve-failure-installs-inert-guard
date: 2026-10-10
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A failed DI resolve of a security or write guard was read as "no guard is
  bound", so the composition root installed the permissive default and the
  guard was silently disabled.
---

## Bug

Six composition sites resolved a guard through
`Injector::optional_resolve_async`, which is `try_resolve_async().ok()`: it
reports "nothing bound it" and "it is bound but the resolve failed" as the same
`None`. Each `None` branch installs a permissive default, so ANY resolve error
turned a wired guard off without a word:

- `InertBoundaryEnforcer` instead of the policy overlay
- `InertNetGuard` instead of the placement gate
- no `WriteTierAuthority` at all, so every block is writable (three sites)
- SQL write-authority reads instead of the Loro authority, which is the
  `convert_block_to_page` parent race

Found by the senior review of holon-work #158 (code audit, not a test), filed
as holon-work #165.

## Root cause

`fluxdi::Injector::optional_resolve_async` discards the error kind
(`fluxdi/src/injector/ts_resolve_async.rs:383` at lock `e6c64618`). At our lock
the discarded kinds are wiring faults — `CircularDependency`, `TypeMismatch`,
`AsyncFactoryRequiresAsyncResolve` — and after the fluxdi head move (#158) a
panicking guard factory joins them as `FactoryFailed`.

The red run
(`docs/Testing/red-logs/2026-10-10-guard-resolve-fails-open-RED.txt`) shows the
boot surviving a `FactoryFailed` resolve of `BoundaryEnforcer`, `NetGuard` and
`WriteTierAuthority`. For `WriteAuthorityReads` it shows the sharper shape: the
dispatcher's shape gate silently took the racy SQL reads and the boot only died
later, at the engine's own fail-loud resolve
(`crates/holon/src/di/registration.rs:878`), with a message naming neither the
shape gate nor the type.

## Missing piece

No test ever made a guard resolve FAIL. The keystone PBT assembles its
container once and that assembly always succeeds, so there is no transition
that reaches "a guard is bound but unresolvable" — the triggering condition is
ungeneratable (COVERAGE). Secondarily, the guards whose absence matters are
bound by app composition roots that the headless keystone container does not
build, so the fail-open branch is not even on its path (ENVIRONMENT).

## Remedy

`holon::di::resolve_if_bound` (`crates/holon/src/di/resolve_if_bound.rs`) keeps
the two facts apart: only `ServiceNotProvided` yields `None`; every other error
panics naming the composition site, the type and the cause. All six sites use
it.

Pinned by `crates/holon/tests/guard_resolve_fails_loud.rs`: one test per guard
type binds that type to a failing factory and asserts the boot dies naming the
site AND the type, plus `an_unbound_guard_still_boots`, which keeps "not bound"
a legal container shape.

Still open: whether the APP container must additionally assert that all four
guard types ARE bound (holon-work #165, "SEC-bound"). Not ruled, so not
implemented; the comment at
`crates/holon/src/api/operation_dispatcher.rs:2720` marks where it would go.
