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

## Real-data cross-check

The measurement example (`examples/measure.rs`) additionally asserts
`parse(render(forest)) == forest` on the forests parsed from three real vault
files (`rt = ok` column in `RESULTS.md`). This caught a second, subtler bug the
generator did not: real `:REQUIRES:` drawer keys collided with the reserved
props namespace. Fixed by (a) giving every reserved props key an `@` sigil that
arbitrary org drawer keys can never carry, and (b) routing `:REQUIRES:` into the
typed edge field in the reader. Both fixes are in the current green tree.
