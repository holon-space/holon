---
id: 2026-10-02-profile-watcher-failure-boots-with-empty-profiles
date: 2026-10-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  If the entity-profile matview watch fails at boot, the DI wiring logs a WARN and
  continues with an empty profile set instead of returning the error, so every entity
  renders without its profile and the user sees no banner.
---

## Bug

A read-only architecture study (2026-10-02, D26.b census, item K8 "silent fallbacks") found
this site. Nothing ran; this comes from reading the code.

## Root cause

In the profile-resolver builder (`crates/holon/src/di/registration.rs:500-551`), the function
returns `Result<Arc<ProfileResolver>>`, but the `Err` arm of `matview_manager.watch(PROFILE_SQL)`
(`:533-551`) does not propagate the error. It logs at DEBUG and WARN and builds a
`ProfileResolver` over an empty `LiveData` (`:538-542`). That is placeholder data. The
function already holds the `ConditionBus` that it uses for `ProfileRefused` (`:555-582`), but
this failure raises no condition. A WARN line in a desktop app's log does not disclose a
degraded mode to the user.

## Missing piece

No test makes the profile watch fail. The keystone and the DI tests boot with a working
matview manager, so the `Err` arm never runs.

## Remedy

OPEN. Rung that closes the gap: a DI-level test that injects a matview manager whose `watch`
fails for `PROFILE_SQL`. It asserts that the builder returns the error, so boot fails loud, or
that a sticky degraded condition is raised. It goes red today because the builder returns `Ok`
with no condition. Fix direction: `?` the error (preferred, fail loud), or raise a condition
on the `ConditionBus` if a degraded boot is wanted.
