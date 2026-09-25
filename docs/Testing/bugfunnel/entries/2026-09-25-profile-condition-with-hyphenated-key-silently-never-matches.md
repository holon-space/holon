---
id: 2026-09-25-profile-condition-with-hyphenated-key-silently-never-matches
date: 2026-09-25
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A profile condition naming a hyphenated property key (`asked-by != ()`)
  parses as `asked - by`, reads two names no row carries, and silently never
  matches; any condition or computed field reading a never-bound identifier
  loaded without an error.
---

## Bug
Reported to the `profile-hyphen-keys` lane: a block-profile variant whose
condition was `asked-by != ()` never rendered, and nothing named the key. The
correct form is `properties["asked-by"]`.

## Root cause
`EntityProfile::build_scope` (crates/holon-api/src/entity_profile.rs:332)
pushes each row column and each flattened property as a Rhai variable. Rhai
parses `asked-by` as the subtraction `asked - by`, so the condition's required
columns are `asked` and `by` (holon-expr `required_columns`). No row carries
them, so `eval_condition` (entity_profile.rs:430) treats the variant as
structurally unbound on every row: a silent non-match, logged only for
declared columns. The same holds for any undefined identifier in a condition
or computed field. Ruling D171.a (refuse unguarded undeclared profile columns
at load) was not implemented anywhere.

## Missing piece
No load-time check compared an expression's free variables with what the
entity's row scope can provide, and no keystone invariant or generator covers
it: `VALID_PROFILE_YAMLS` holds only well-formed profiles.

## Remedy
`holon_profiles::check_profile_scope` (crates/holon-profiles/src/profile_scope.rs)
refuses a profile whose computed field or variant condition reads, without an
`is_def_var("x")` or `x != ()` guard, a name that is not a type field, an edge
column (block), a computed field or a UI-state variable. The error names the
profile, variant/field, expression and names, and suggests
`properties["<hyphenated-key>"]` when the names are segments of one. It runs in
`TypeRegistry::apply_parsed_profile` (bundled and kitchen profiles) and on
every org-embedded profile block (`TypeRegistry::profile_scope_check`, wired in
crates/holon/src/di/registration.rs). Pinned by
crates/holon-profiles/tests/profile_scope_refusal.rs. All shipped profiles
under assets/default/types/ load unchanged.
