---
id: 2026-08-22-case-varying-type-names-share-one-table
date: 2026-08-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Two datatypes whose names differ only in case both registered successfully and
  silently shared one raw table and one matview, each reporting a table name that
  storage had already discarded.
---

## Bug

Registering a type named `Person` and then one named `person` both returned
`Ok`, each reporting its own raw table (`Person_raw`, `person_raw`), while the
schema held only ONE set of objects. The second type was bound to the first
one's columns with no error anywhere.

Found by self-review inside the turso re-pin lane, not by a test — while
reasoning about what the newly-widened identifier rule now admitted. The
regression was introduced in that same lane and never reached `main`.

Reproduced with the guard reverted (`lane-logs/red-on-revert.log`):

```
PROBE register Person: Ok("Person_raw")
PROBE register person: Ok("person_raw")
PROBE object table "__turso_internal_dbsp_state_v1_person"
PROBE object view  "person"
PROBE object table "person_raw"
```

Two successes, two claimed table names, one actual table.

## Root cause

The Turso serialization CANONICALIZES a type's names to lowercase. A type
declared `Person` reaches the schema as `person_raw` / view `person` /
`__turso_internal_dbsp_state_v1_person` — the declared spelling is discarded
before `sqlite_master` ever sees it.

Above storage the two names stay distinct: `TypeRegistry` keys its map on the
raw `String` (crates/holon-profiles/src/type_registry.rs:43) and `EntityName`
compares case-sensitively (crates/holon-api/src/types.rs:29,36). The raw table
is created with `CREATE TABLE IF NOT EXISTS`
(crates/holon/src/core/queryable_cache.rs:1301), so the second registration is a
no-op that reports success.

The lane had removed `reject_non_lowercase_name` and widened
`reject_non_identifier_name` from `[a-z][a-z0-9_]*` to `[A-Za-z][A-Za-z0-9_]*`,
on the premise that the pinned engine's new identifier handling made mixed-case
type names safe. The engine half of that premise is true and stays true — SQL
spelled in any case resolves to the same table, pinned by
`mixed_case_sql_resolves_to_the_lowercase_type`. The premise was wrong about the
NAME: a mixed-case type name never reaches storage at all, so the widening
bought nothing and admitted a state nothing downstream could diagnose.

Because the distinguishing spelling is discarded before `sqlite_master`, a
guard that looks for "a fold-equal name spelled differently" cannot work — it
false-positives on ordinary re-registration and false-negatives on the real
collision. Both directions were measured before the approach was abandoned.

## Missing piece

No test and no generator ever declares two type names that fold to the same
identifier. `DeclareTypedSchema` draws `gen_<n>` from a counter of declared
types, so every drawn name is distinct by construction and a colliding pair is
unreachable — the keystone could not have generated this interaction. The
adapter's own unit tests each registered a single mixed-case type in isolation,
which passes whether or not a second spelling would collide.

The oracle was not the weakness: had a case reached the state,
`inv-typed-matview-matches-ref` would very likely have gone red on the second
type's columns.

## Remedy

`reject_non_lowercase_name` restored and `reject_non_identifier_name` narrowed
back to `[a-z][a-z0-9_]*`, both now documenting the measured, PERMANENT reason
(storage canonicalizes; a non-lowercase name is not representable) rather than
the old "temporary, pending an engine fix" wording. The illegal state is
unrepresentable again, so the guard is stateless and needs no schema lookup.

The generator's type-name and column-name case draws were reverted for the same
reason — capitalization is a distinction storage does not keep, so drawing it
would only bounce cases off the refusal. The generator is now BOTH structurally
unable to draw a colliding pair (single `gen_<n>` counter, lowercase alphabet)
AND blocked by `register` if it ever tried; the case dimension is pinned next to
the adapter instead, by `mixed_case_sql_resolves_to_the_lowercase_type`
(mixed-case and quoted SQL against a lowercase-registered type) and
`a_mixed_case_type_is_rejected_at_registration`.

Note the two guards deliberately overlap: the narrowed shape rule already
excludes uppercase, but `reject_non_lowercase_name` runs first so a case
violation gets the accurate diagnosis instead of the shape rule's
hyphen-and-leading-digit message.

Open follow-up, recorded outside this entry: if case-PRESERVING type names are
ever wanted, uniqueness under case folding has to be enforced at `TypeRegistry`,
where the declared spelling still exists. That is a registry design question,
not an adapter one.
