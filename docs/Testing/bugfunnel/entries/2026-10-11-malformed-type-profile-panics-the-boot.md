---
id: 2026-10-11-malformed-type-profile-panics-the-boot
date: 2026-10-11
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A malformed type profile (a variant condition that does not split, or a
  render that does not parse) made `profile_from_type_def` panic, so the boot
  stopped.
---

## Bug
Found outside an automated test: the places experiment and the split-condition
fix (holon-space/holon-work#164) showed that a refused condition reached a
panic. Two shapes stopped the boot:

- a variant condition that does not split, for example
  `flagged || archived && is_focused` (a top-level `||` beside a top-level
  `&&`)
- a variant render that does not parse

## Root cause
`profile_from_type_def` in `crates/holon-profiles/src/lib.rs` (around lines
707 and 823) turned the error of the condition splitter and the render parser
into a panic ("Failed to parse profile variants for entity '<type>'"). Both
boot paths build all type profiles once, so one bad type stopped the whole
boot. This broke the directive that Holon boots under all circumstances.
Red log: `target/profile-panic-red.log`.

## Missing piece
No generator draws a malformed type profile. The keystone PBT authors no
profile conditions or renders, and the existing profile tests only fed
well-formed input, so no generated interaction could reach this state.

## Remedy
Commit b9ccb4bd (PROFILE-panic.a): `profile_from_type_def` returns
`Result<Option<EntityProfile>, MalformedTypeProfile>`. The typed refusal names
the type, origin, variant, part, text and reason. The new condition
`ConditionKind::TypeProfileRefused` discloses it (Error, toast). Both boot
paths build profiles through `type_profiles_from_registry`, which leaves the
refused type out and raises the condition. The type keeps its schema and rows
and renders with the default profile.

Pinned by:
- `crates/holon-app/tests/malformed_type_profile_boots_degraded.rs` (real boot
  with a condition that does not split, a render that does not parse, and a
  well-formed type)
- `crates/holon-profiles/tests/malformed_type_profile_refusal.rs` (typed
  refusals, bus disclosure, every bundled type profile parses)

Open: a vault type profile that adds a bad variant to an existing type would
refuse the whole type; PROFILE-home.a refuses a vault profile as a unit.
