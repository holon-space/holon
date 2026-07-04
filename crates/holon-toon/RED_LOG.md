# Round-trip PBT — red-for-the-right-reason log

Per the `holon-feature` discipline, the round-trip property
(`parse(render(forest)) == forest`, `tests/roundtrip_proptest.rs`) was driven
red before its covering implementation was trusted. This crate is an
experiment, so keystone integration is intentionally skipped; the red below
proves the load-bearing part of the design — the list-escaping codec — actually
bears load.

## Red: the list codec is necessary

**Change under test:** revert the multi-valued props fields (`@tags`, `@req`,
`@adv`) from the dedicated list codec (`encode_list`/`decode_list`, which
escapes element commas) back to the naive `tags.join(",")` / `value.split(',')`.

**Command:**

```
PROPTEST_CASES=2000 cargo test -p holon-toon --test roundtrip_proptest parse_render_is_identity
```

**Result: FAILED**, shrunk to the minimal counterexample of a single tag whose
value is one comma:

```
input   tags: [","]
parsed  tags: ["", ""]
```

**Why this is the right reason:** a comma *inside* a tag is indistinguishable
from the list separator under naive comma-join/split, so `","` round-trips to
two empty tags. This is exactly the ambiguity `encode_list` exists to remove
(it escapes element commas as `\,` one layer beneath the props escaping). The
failure localizes to the list layer, not to unrelated machinery — a targeted
red, not a smoke alarm.

## Green

Restoring `encode_list`/`decode_list`:

```
cargo test -p holon-toon --test roundtrip_proptest
running 2 tests
test render_always_parses ... ok
test parse_render_is_identity ... ok
test result: ok. 2 passed; 0 failed; ...
```

`parse_render_is_identity` runs 400 cases (2000 in the red run) over a generator
covering: task states, priorities, multi-tag sets, arbitrary drawer props,
`requires`/`advice` id-lists, scheduling, Text/Source/Image kinds, multi-line
source bodies, and adversarial content built from the exact characters that
clash with TOON's syntax (`: , " [ ] { } \`, tab, newline, leading `-`/`#`,
non-ASCII). See `tests/roundtrip_proptest.rs`.

## Generic tabular codec — round-trip PBT (`tests/table_proptest.rs`)

The generic layer (`src/table.rs`) has its own round-trip property,
`Table::parse(table.render()) == table`, over generated row sets with awkward
column keys, values across every `ToonValue` variant, heterogeneous rows
(each column independently present/absent), and empty/zero-column tables.

### Red 1 (organic — found while writing the codec): zero-column rows dropped

**Symptom:** the first PBT run failed immediately:

```
parse failed: declared row count 1 does not match the 0 data rows that followed the header
minimal failing input: Table { name: "rows", columns: [], rows: [ {} ] }
```

**Root cause:** a zero-column table renders each row as a blank indented line
(`"  "`), but the parser filtered blank lines out (`!l.trim().is_empty()`),
losing the rows. **Fix:** the generic parser no longer filters blank lines — a
`render`ed document has no incidental blanks, so every post-header line is a
data row. This is the right reason: the failure localized precisely to the
blank-line handling, and the fix is the minimal change that reconciles
"zero-column rows are legitimately blank" with the row-count invariant.

### Red 2 (seeded — the absent-vs-empty confusion): empty string ≡ absent

**Change under test:** make the generic string encoder collapse the empty
string to a bare empty cell (`encode_str("") -> ""`), the way the *block*
schema's `encode_cell` intentionally does — i.e. erase the absent-vs-empty
distinction this codec exists to preserve.

**Command:**

```
PROPTEST_CASES=2000 cargo test -p holon-toon --test table_proptest table_roundtrip_is_identity
```

**Result: FAILED**, shrunk to the minimal counterexample:

```
minimal failing input: table = Table {
    columns: [ "" ],
    rows: [ { "": Str("") } ],
}
parse failed: row 0: expected 0 comma-separated cells, found 1
```

**Why this is the right reason:** with `Str("")` rendered as an empty cell it is
indistinguishable from an *absent* key, so a single `""`-valued cell (and,
because header fields share the encoder, an empty *column name*) round-trips to
nothing. This is exactly the ambiguity the explicit `""` token removes. The
failure localizes to the value/absence boundary, not unrelated machinery.

**Green:** restoring `encode_str("") -> "\"\""` makes both properties pass at
2000 cases:

```
running 2 tests
test render_always_parses ... ok
test table_roundtrip_is_identity ... ok
test result: ok. 2 passed; 0 failed; ...
```

## Real-data cross-check

The measurement example (`examples/measure.rs`) additionally asserts
`parse(render(forest)) == forest` on the forests parsed from three real vault
files (`rt = ok` column in `RESULTS.md`). This caught a second, subtler bug the
generator did not: real `:REQUIRES:` drawer keys collided with the reserved
props namespace. Fixed by (a) giving every reserved props key an `@` sigil that
arbitrary org drawer keys can never carry, and (b) routing `:REQUIRES:` into the
typed edge field in the reader. Both fixes are in the current green tree.
