---
id: 2026-10-05-runtime-type-escapes-typed-computed-override-check
date: 2026-10-05
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A type registered at runtime with a typed computed field escaped the D66.a
  refusal, so an already-loaded vault profile could replace that field with a
  Rhai script on the live seat alone.
---

## Bug
Found by the verifier of lane d45-followups round 1. `profile_load_check`
(crates/holon-profiles/src/type_registry.rs) copied the type map once, at boot.
A type registered afterwards (`TypeRegistry::register`, reached by
`declare_type` and the declare_type op/MCP) could add a typed computed field
that a loaded vault profile already redeclared. `merge_profile` then let the
Rhai field win on the live seat only, which D66.a forbids.

## Root cause
Two staleness shapes. The check closure read a boot snapshot. And nothing
remembered what loaded profiles declare, so a type that arrived second was
never compared with them. The older `check_profile_scope` check shared the
snapshot: a profile loaded or edited after a type was declared was checked
against a map without that type, and refused wrongly.

## Missing piece
The D66.a test loaded the profile against the types present at boot and never
registered a type afterwards. No keystone generator declares types at runtime
together with vault profiles.

## Remedy
The check closure reads the live type map, and records each accepted profile's
computed field names (a refused load releases its record). `register` refuses a
type whose typed computed field a recorded profile redeclares, with
`TypedComputedFieldOverride`. Tests: the three added to
crates/holon-profiles/tests/profile_typed_computed_override_refusal.rs (red
log lane-logs/d66-red.log). A profile block that leaves the vault releases its record
when the resolver stops applying it (see the release legs below).
Test: crates/holon-app/tests/deleted_vault_profile_releases_its_claim.rs (red
log lane-logs/d66b-red.log: the type stayed refused after the profile's
document was deleted).

A refused profile EDIT keeps the older version's record: the mirror drops the
refused row and the older version stays applied (measured by
`live_data::tests::an_unparseable_update_keeps_the_earlier_version`, log
lane-logs/d66c-measure.log), so the load check no longer releases a record on
refusal. Test: `a_refused_edit_leaves_the_live_profiles_claim_in_force`, run
inside `a_vault_profile_claim_follows_the_live_version_of_its_block` (red log
lane-logs/d66c-red.log: the type registered although the older version was still
applied).

### Race leg: check and record were two critical sections
The verifier of round 4 showed that `register` (claims read, release, types
write) and the profile load check (types read, release, claims write) both
accepted when they ran at the same time: 2972 of 3000 trials. The registry
then held a typed `display_name` while the mirror held a Rhai `display_name`
for the same entity. The types and the claims now live in ONE struct under ONE
lock (`Admitted`, crates/holon-profiles/src/type_registry.rs), and each side
checks and records inside one critical section. Test:
`a_type_and_a_profile_admitted_concurrently_are_never_both_accepted`
(crates/holon-profiles/tests/profile_typed_computed_override_refusal.rs). Red
log lane-logs/d66f-red.log (500 of 500 trials accepted both), green log
lane-logs/d66f-green.log.

### Release leg: the claim left in a spawned task
The claim release ran in a spawned task on the mirror's signal map, so a
`declare_type` right after the deletion could still meet the stale claim, and
the test hid it behind a 30 s poll. `LiveData::on_delete` now runs the release
inside the delete step, while the map is write-locked and before the removal is
readable (crates/holon-api/src/live_data.rs). Tests:
`the_delete_hook_has_run_by_the_time_the_removal_is_observable` and
`the_delete_hook_runs_for_a_key_the_mirror_never_held` (red log
lane-logs/d66f-red2.log, with the hook stubbed to a no-op); and
`a_deleted_vault_profile_stops_refusing_the_type_it_overrode`, which now waits
for the profile to leave the resolver and then asserts the FIRST `register`
succeeds. The app-level test cannot go red on the old task-based release: six
runs against it passed, because the resolver cache lags the mirror, so the
ordering guarantee is pinned by the LiveData tests.

### Stale `ProfileRefused` condition
A `ProfileRefused` condition was never cleared when the refused block was
deleted: no accepted load follows, so nothing cleared it. The same delete hook
now clears it. Test: `deleting_a_refused_profile_block_clears_its_refusal`
(crates/holon-app/tests/deleted_vault_profile_releases_its_claim.rs); red log
lane-logs/d66f-red3.log (condition still raised 30 s after the delete).

### Release leg 2: the claim left before the profile stopped computing
The verifier of round 5 (lane-logs/d45-followups-verify-3.md) raced a spinning
`register` against an awaited `block.delete`. The delete hook released the
claim at the EARLIEST step of the deletion, but the resolver kept computing the
Rhai `display_name` until its spawned cache rebuild swapped in a cache without
the profile. A `register` accepted in between left two computations for one
field: 4 of 4 runs. The claim now means "this profile is in effect in the
resolver". `VaultProfileClaims::apply`
(crates/holon-profiles/src/type_registry.rs) swaps the resolver cache in and
sets the claims to exactly the applied profiles, under the registry lock, while
`ProfileResolver::apply_source` (crates/holon-profiles/src/lib.rs) holds the
profile mirror read-locked, so a rebuild cannot apply an older mirror state
after a newer one. The load check still adds an accepted profile's claim before
it can take effect, and an accepted edit keeps the replaced version's claim
until the resolver applies the edit. The delete hook only clears
`ProfileRefused`; it is installed before `subscribe` (a later install panics)
and a panic inside it is logged and does not end the mirror's CDC actor.
Tests: `a_deleted_vault_profile_stops_refusing_the_type_it_overrode` now races
a spinning `register` against the delete and asserts the profile is not in
effect at the instant `register` is accepted (red log lane-logs/r6-red-5x.log,
4 of 5 runs failed; green lane-logs/r6-green-5x.log);
`an_edit_dropping_the_field_releases_the_claim_when_the_resolver_applies_it`
(crates/holon-profiles/tests/profile_typed_computed_override_refusal.rs);
`a_delete_hook_cannot_be_installed_after_subscribe` and
`a_panicking_delete_hook_leaves_the_subscribe_actor_alive`
(crates/holon-api/src/live_data.rs).
