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

Eleven composition sites resolved a guard through
`Injector::optional_resolve_async` (or its sync twin `try_resolve().ok()`, or
`try_resolve_all_async().unwrap_or_else(|_| vec![])`), all of which report
"nothing bound it" and "it is bound but the resolve failed" as the same
absence. Each absence branch installs a permissive default, so ANY resolve
error turned a wired guard off without a word:

- `InertBoundaryEnforcer` instead of the policy overlay
- `InertNetGuard` instead of the placement gate
- no `WriteTierAuthority` at all, so every block is writable (three sites)
- SQL write-authority reads instead of the Loro authority, which is the
  `convert_block_to_page` parent race (three sites)
- zero `OperationObserver`s, and the only one prod binds IS the operation log,
  so undo and the audit trail both went away
- the read-only document authority absent, so every block homed in a
  read-only-format document became editable again
- reduced `SessionCapabilities` pinned for the whole session, and the SqlOnly
  write leg chosen, off a failed `BlockCellRegistry` / `CrudAuthority` resolve

Found by code audit, not by a test: the senior review of holon-work #158 found
the first six, and the security verifier of the fix found the rest. Filed as
holon-work #165.

## Root cause

`fluxdi::Injector::optional_resolve_async` discards the error kind
(`fluxdi/src/injector/ts_resolve_async.rs:383` at lock `e6c64618`). At our lock
the discarded kinds are wiring faults — `CircularDependency`, `TypeMismatch`,
`AsyncFactoryRequiresAsyncResolve` — and after the fluxdi head move (#158) a
panicking guard factory joins them as `FactoryFailed`.

The red runs are `docs/Testing/red-logs/2026-10-10-guard-resolve-fails-open-RED.txt`
and `...-round2-RED.txt`. They show the boot surviving a `FactoryFailed`
resolve of `BoundaryEnforcer`, `NetGuard`, `WriteTierAuthority` and the
observer set, and the shape gate silently taking the racy SQL reads while the
boot died later at `crates/holon/src/di/registration.rs:878` with a message
naming neither the gate nor the type.

## Missing piece

No test ever made a guard resolve FAIL. The keystone PBT assembles its
container once and that assembly always succeeds, so there is no transition
that reaches "a guard is bound but unresolvable" — the triggering condition is
ungeneratable (COVERAGE). Secondarily, the guards whose absence matters are
bound by app composition roots that the headless keystone container does not
build, so the fail-open branch is not even on its path (ENVIRONMENT).

## Remedy

`holon_core::resolve_if_bound_async`, `resolve_if_bound` and
`resolve_all_if_bound_async` (`crates/holon-core/src/resolve_if_bound.rs`) keep
the two facts apart: only `ServiceNotProvided` yields the absence; every other
error panics naming the composition site, the type and the cause. The helper
lives in `holon-core`, next to the guard traits, so the org-mode and Loro
composition roots reach the same rule as the engine's. All eleven sites use it.

Pinned by `crates/holon/tests/guard_resolve_fails_loud.rs` (the five
`OperationModule` sites plus `an_unbound_guard_still_boots`, which keeps "not
bound" a legal container shape) and
`crates/holon-app/tests/guard_resolve_fails_loud_in_the_app.rs` (the
event-infra and org-mode seam sites through the real app wiring).
`BootError::from_bootstrap_error` gained an `Operations` component so a guard
wiring fault is not attributed to a store that is fine
(`crates/holon-app/src/boot_error.rs`).

SEC-bound.a (ruled 2026-10-10) closes the other half: the binding is no longer
optional. `crates/holon/src/di/guard_bindings.rs` settles every container's
guard bindings before anything resolves the dispatcher — a composition root
that declares `RootOwnsGuardComposition` (the app) must bind every guard
itself and a missing one stops the boot naming it, while every other container
gets the EXPLICIT no-op guards. The dispatcher then resolves all four as
REQUIRED, so it has no permissive branch left at all, and the arms that used to
install an inert guard are gone.

That also closes the residual the verifier noted: a guard bound in the wrong
scope reads as absent in the scope the consumer resolves from, and in the app
container absent is now fatal rather than inert.

Red log for this half:
`docs/Testing/red-logs/2026-10-10-guard-unbound-in-app-container-RED.txt` — one
removed binding in `crates/holon-app/src/wiring.rs` stops every app boot,
naming the guard.

Still later, not open: SEC-boot.a ratified the panic as the stopgap, with a
typed error that boot discloses as the eventual shape
(`crates/holon/src/di/registration.rs:905-915` names that step).
