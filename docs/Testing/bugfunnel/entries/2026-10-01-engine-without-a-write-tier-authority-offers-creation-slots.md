---
id: 2026-10-01-engine-without-a-write-tier-authority-offers-creation-slots
date: 2026-10-01
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  A `ReactiveEngine` built without a write-tier authority treats every parent as writable, and the DI factory silently accepts the authority's absence.
---

## Bug
Found by the I1 verifier (code audit). `tier_allows_creation_under` returns `true`
for `write_tier == None`; `register_render_services` swallows `ServiceNotProvided`.

## Root cause
The authority is provided only inside the vault-root branch of
`crates/holon-app/src/wiring.rs` (the `orgmode_root` branch). Engines are also built
without it: a session with no vault root (a disclosed, configured mode) and two
direct `ReactiveEngine::new` sites (`frontends/holon-worker/src/lib.rs`,
`frontends/gpui/src/lib.rs` fallback). The `None` arm is therefore reachable in
real configurations, so making it fail would break them.

## Missing piece
No test proved the GPUI DI wiring provides the authority.

## Remedy
Round 1: `write_tier_wiring_tests::a_vault_backed_session_provides_the_write_tier_authority`
(frontends/gpui/src/di.rs) fails when the wiring stops providing it (teeth by sabotage).

Round 2: the authority is a required `ReactiveEngine::new` parameter, so the `None`
arm no longer exists. Configurations without read-only documents (no vault root in
`wiring.rs`, `register_block_query_frontend`, `holon-worker`) provide
`holon_core::NoReadOnlyDocuments`. `register_render_services` panics when the DI
container lacks the authority. The GPUI fallback `ReactiveEngine::new` was reachable
only through `launch_holon_window`, which nothing called; both were deleted.
Red: lane-logs/i1b2-red.log (`None` allowed a create; resolution without the service
passed). Green: lane-logs/i1b2-green.log. Teeth (fail-open default restored in
`resolve_write_tier`): lane-logs/i1b2-teeth.log.

Round 3 (same class, second consumer; extends this entry instead of adding one): the
I1b verifier found `BlockCellRegistry` kept the authority as an `Option` and allowed
every create/content write when absent, and `LoroModule` installed it with
`optional_resolve_async`. The registry now takes the authority in `with_loro` /
`with_loro_doc` (no setter), and `LoroModule` resolves it with `resolve_async`, so a
root without it fails at resolution. `holon-app` wiring on wasm32 provides
`NoReadOnlyDocuments` (the native-only providers left render services without one).
Red: lane-logs/i1b3-red.log (registry with no authority allowed a create under a
read-only parent). Green: lane-logs/i1b3-green.log
(`the_cell_registry_cannot_be_resolved_without_a_write_tier_authority`). Teeth
(substituting `NoReadOnlyDocuments` in the wiring): lane-logs/i1b3-teeth.log.
