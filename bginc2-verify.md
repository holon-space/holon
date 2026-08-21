# BG Increment 2 — delta re-check (items 1 & 2)

`pwd` for every verdict: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/bg-inc1`

**Both items CONFIRMED.** One process finding: the pin I was given does not
match the tree.

---

## P1 — THE PIN IS STALE (read this first)

The pin `34d5341cde97` is **not** the current working copy. `@` now resolves to
**`f85bebdf7991`**.

Four files differ between the pin and disk:

| File | at pin | on disk |
|---|---|---|
| `crates/holon/src/core/type_declaration.rs` | 106 lines | **186 lines** |
| `crates/holon/src/api/operation_dispatcher.rs` | 1838 | 1840 |
| `crates/holon-turso/src/turso_adapter.rs` | 1074 | 1080 |
| `…/pbt/transitions/declare_typed_schema.rs` | 178 | 179 |

The 80-line difference in `type_declaration.rs` is **the entire behavioural
pinning test module for item 1** — it does not exist in the revision you pinned.
Source mtimes are 03:52:08–03:53:11.

**The lane's RESUME2 gate logs finished at 03:47:23–03:49:50** — roughly three
minutes *before* those files were last written. So "gates re-run green by the
lane" does not cover the tree as it now stands. Same family as the stale-witness
hazard.

**Mitigation, and why I still sign off.** I diffed `34d5341c → @` and the
post-pin delta is confined to: doc comments, one `#[cfg(test)] mod tests`, and
one error-message string (whose test was updated in step and strengthened). **No
production control flow changed after the pin** — `reject_non_identifier_name`
and its position in the chain were already at `34d5341c`, which the gates did
cover. I then re-ran every affected test myself against the **final disk tree**
(all green, listed at the end).

**Action:** re-pin to `f85bebdf7991` before weaving. A full gate re-run is
optional given the delta's shape; your call.

---

## Item 1 — honest contract: **CONFIRMED** (exceeds the bar)

### Error text — no impossible instruction

`register_provider` (`operation_dispatcher.rs:229`) now reads:

> `entity '{entity}' already has a write authority; registering a second one would make the routing scan decide which of the two a write lands in. Re-declaring a live type is NOT SUPPORTED in this increment: this registry is append-only, and TursoAdapter::teardown drops only the SQL artifacts, so no sequence of calls frees the name. Declaring over a live type arrives with the migrate primitive (OQ-5), which retires this error. Until then, use a name that is not yet declared.`

`Tear the type down before re-declaring it` is gone. Append-only reason present,
OQ-5 named, and it closes with an instruction that actually works. ✓

### Docs — claims scoped

- Module doc gained `DECLARATION IS ONE-WAY IN THIS INCREMENT`, naming both
  causes (append-only registry; teardown is SQL-only) and stating
  `declare → teardown → declare` does not round-trip. ✓
- `declare_type`'s fn doc no longer advertises the OQ-5 primitive; it now says
  dropping artifacts "does NOT undeclare the type" and re-declaring "fails and
  stays failed." ✓
- The ordering comment is scoped: "That guarantee covers THIS step only — a
  failure at step 3 refuses the declaration with the registry already mutated,
  which is unrecoverable for that name." That is exactly the overclaim I
  flagged, corrected. ✓

### The pinning test — meets the behavioural bar

There are two, correctly split, and the split is documented:

1. `the_duplicate_authority_error_does_not_promise_a_recovery_path`
   (`operation_dispatcher.rs`) — pins the **wording** (asserts presence of
   "NOT SUPPORTED in this increment", "append-only", "OQ-5"; asserts **absence**
   of "Tear the type down"). Its doc now says explicitly that it pins wording
   only and points at the behavioural test. Teeth: a revert of the wording fails
   it.

2. `a_declared_type_cannot_be_redeclared_even_after_teardown`
   (`type_declaration.rs`) — **this is the one that meets my bar**, and it meets
   it in full. It uses the real `declare_type` against a real
   `TursoBackend::new_in_memory`, a real `TypeRegistry` and a real
   `OperationDispatcher` — no mocks. It asserts, in order:
   - after declare #1: `registry.contains("gen_1")` **and**
     `dispatcher.has_provider("gen_1")` — the observed state, not a string;
   - declare #2 → `Err`, **attributed to step 3** via
     `err.contains("registering the write authority failed")`, so it
     discriminates *which* step refused rather than accepting any error;
   - `!err.contains("Tear the type down")`;
   - calls the real `TursoAdapter::teardown`, then asserts
     `registry.contains` **and** `has_provider` are **still true** — the leftover
     state I asked to see pinned;
   - declare #3 after teardown → `Err`, failing identically.

   That is the documented one-way property executed end to end, which is what
   the earlier draft was missing. My previous pass's shortfall is **closed**.

I ran it: `cargo test -p holon --lib type_declaration` →
`a_declared_type_cannot_be_redeclared_even_after_teardown ... ok`, 1 passed.

---

## Item 2 — shape check: **CONFIRMED**

### Chain position and ordering

`turso_adapter.rs:320-323`:

```rust
Self::reject_keyword_identifiers(type_def)?;
Self::reject_non_lowercase_name(type_def)?;
Self::reject_non_identifier_name(type_def)?;   // third
```

Case runs **before** shape, so a mixed-case name still gets the specific IVM
message rather than the generic shape one. Confirmed empirically, not by
reading: `a_mixed_case_type_is_rejected_at_registration` still passes, and that
test asserts on `"silently stops matview maintenance"`. ✓

### The rule

`[a-z]` first char, then `[a-z0-9_]*` — implemented with `is_ascii_lowercase` /
`is_ascii_digit` / `'_'`. An empty name is also rejected (`chars.next()` is
`None`). No wider alphabet admitted. ✓

### The three-shape test

`a_name_that_is_not_a_bare_identifier_is_rejected_at_registration` covers
`"gen-1"`, `"1x"`, `"a b"` — hyphen, leading digit, embedded space, all
lowercase so they clear the case rule and genuinely reach this one. Plus the
no-artifacts-left check (`sqlite_master` query) and the `gen_1`-still-legal
positive arm. It gained a third assertion after the pin, requiring the error to
scope itself to the serialization. ✓

**Teeth confirmed by construction:** deleting `reject_non_identifier_name` would
let `gen-1` reach the DDL parser, which errors with a syntax message that does
**not** contain `"cannot be serialized to Turso"` or `"[a-z][a-z0-9_]*"` — so
the test fails rather than passing vacuously. The `gen_1` arm gives teeth in the
other direction against an over-narrow rule (e.g. `[a-z]+`).

### Doc rationale — deviates from your spec, and I think the lane is right

You specified a **PERMANENT** rationale (unquoted DDL + URI scheme +
non-injective fold). The lane reframed it as **TEMPORARY**, expiring with the
same engine fix as its two neighbours, and dropped the URI-scheme argument.

**I judge the lane's reframe more correct than my own F4 wording.** My F4 note
claimed the URI-scheme constraint was independent permanent grounds. That is
**wrong for hyphens**: RFC 3986 allows `-` in a scheme after the first
character, so `gen-1` is a perfectly valid URI scheme. The lane caught my error
and says so explicitly ("`TypeRegistry` deliberately carries hyphenated types,
and a hyphen is a perfectly good URI scheme"). Good catch on their part.

**One residual doc-precision point** (minor, no code change needed): the
"expires with the engine fix" framing is right for hyphens but **overclaims for
the other two shapes**. A leading digit (`1x`) and an embedded space (`a b`) can
never be valid URI schemes under RFC 3986 regardless of what the fork does, so
`EntityName::new`'s scheme guard would still reject them after the fix. If
someone reads "lifted by the same engine fix" and deletes the whole check when
the fork lands, `1x` and `a b` would then hit the `debug_assert` in
`EntityName::new`. One clause noting that the leading-digit/space half survives
the fix would close it. Same overclaim appears in the error string's closing
sentence. Not worth blocking on.

### Scope of the check is correct (I probed for a hole and found none)

The rule checks the type **name** only, not field names. That is right, not a
gap: `TypeDefinition::to_create_table_sql` emits every column as
`"{name}" {type}` — **quoted** — and `matview_select` likewise quotes each
field. Only the table/type name is interpolated unquoted. Keyword field names
are separately covered by `reject_keyword_identifiers`.

---

## Drift check: **CLEAN**

- File set at `@`: **22** — 21 modified + `crates/holon/src/core/type_declaration.rs`
  added. Identical to my previous pass's set minus `run-fmt-check.sh`. ✓
- `run-fmt-check.sh`: **deleted** (`D` in the delta) and absent from disk. ✓
- `bginc2-verify.md`: absent from disk and untracked. ✓
- Disk byte-matches `@` for all 22 files. ✓
- Delta `227c8950 → @` touches exactly the 4 files you scoped plus the
  `run-fmt-check.sh` deletion. **No other file drift.** ✓

---

## What I ran against the final disk tree (`f85bebdf7991`)

| Command | Result |
|---|---|
| `cargo test -p holon-turso --lib turso_adapter` | **13 passed, 0 failed** (was 12; the new shape test, and `a_mixed_case_type_is_rejected_at_registration` still green) |
| `cargo test -p holon --lib operation_dispatcher` | **14 passed, 0 failed** (incl. the wording test) |
| `cargo test -p holon --lib type_declaration` | **1 passed** — the one-way behavioural pin |
| `cargo fmt --all -- --check` | exit 0, empty |
| byte-compare disk vs `@`, all 22 files | no differences |

## Verdict

| Item | Verdict |
|---|---|
| 1 — honest contract (error, docs, pinning test) | **CONFIRMED** — behavioural pin exceeds the bar I set |
| 2 — shape check (position, rule, tests, doc) | **CONFIRMED** — one minor doc-precision note |
| No other file drift | **CONFIRMED** |
| Pin integrity | **STALE** — re-pin to `f85bebdf7991` before weaving |
