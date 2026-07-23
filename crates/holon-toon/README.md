# holon-toon (experiment)

Two things live here:

1. A **generic tabular TOON codec** (`src/table.rs`) — TOON's actual sweet
   spot: uniform, value-heavy rows (e.g. `holon` MCP `execute_query` /
   `execute_raw_sql` results) where emitting each column name once in a header
   beats JSON's repeated per-row keys. This is the reusable, block-independent
   part and is wired into the MCP query tools as the default output format (`format: "json"` is the opt-out).
2. The original **block-forest projection** — a dense, round-trippable TOON
   projection of the Holon block forest, built as a structural sibling of
   [`holon-org-format`](../holon-org-format). This is the part the experiment
   evaluated and **rejected** for block forests (net-negative vs ID-compressed
   org — see `RESULTS.md`); it is kept as a second, fixed-schema instantiation
   over the same escaping primitives.

Both funnel through **one** escaping implementation (`src/toon.rs`).

## Layout

| File | Role |
|---|---|
| `src/toon.rs` | The single escaping layer: scalar quoting/escaping, row split, props + list sub-codecs |
| `src/table.rs` | **Generic tabular codec**: `Vec<Row>` (`Row = BTreeMap<String, ToonValue>`) ⇄ TOON, sorted column union, explicit absent-vs-empty |
| `src/schema.rs` | The fixed 6-column *block* schema (`blocks[N]{id,depth,state,props,body,title}`) |
| `src/models.rs` | Block-forest domain types (`ToonBlock`, `BlockNode`, `Forest`) — parse-don't-validate newtypes mirroring `holon_api::Block` |
| `src/renderer.rs` | `Forest` → TOON |
| `src/parser.rs` | TOON → `Forest`, fail-loud (`Result` everywhere) |
| `src/org_reader.rs` | Minimal Org reader + two Org renderers, for the measurement only |
| `tests/table_proptest.rs` | Generic-codec round-trip PBT (`Table::parse(t.render()) == t`) |
| `tests/table_units.rs` | Generic-codec golden + absent-vs-empty + nested-JSON + error paths |
| `tests/roundtrip_proptest.rs` | Block-forest round-trip PBT (`parse(render(x)) == x`) |
| `tests/units.rs` | Block-forest golden render + fail-loud error paths |
| `examples/measure.rs` | Token measurement: org vs TOON on real files, and JSON vs TOON on synthetic query results (`measure synthetic`) |

## Generic codec at a glance

```rust
use holon_toon::{Table, ToonValue, Row};
use std::collections::BTreeMap;

let rows: Vec<Row> = vec![
    BTreeMap::from([("id".into(), ToonValue::Int(1)),
                    ("name".into(), ToonValue::Str("alice".into()))]),
    BTreeMap::from([("id".into(), ToonValue::Int(2))]), // `name` absent, not empty
];
let table = Table::from_rows("rows", rows)?;   // columns = sorted key union
let text = table.render()?;                    // -> "rows[2]{id,name}:\n  1,alice\n  2,\n"
assert_eq!(Table::parse(&text)?, table);       // lossless round-trip
```

- **Columns**: lexicographically-sorted union of all row keys (deterministic;
  rows are unordered maps, so there is no meaningful first-seen order).
- **Absent vs empty**: a missing key renders as an empty cell and parses back as
  **absent**; an empty *string* renders as the explicit token `""`. `Null` is
  the bare literal `null`. All three are distinct and round-trip.
- **Nested JSON** (SQL/JSON columns): carried as a JSON **string in the cell**
  (`ToonValue::from_json`, behind the `serde-json` feature) — lossless bytes;
  the "was-JSON" type is not recovered on parse (documented limitation).

## Docs

- **[MAPPING.md](MAPPING.md)** — per-construct org→TOON mapping & verdicts.
- **[RESULTS.md](RESULTS.md)** — token measurement table & recommendation.
- **[RED_LOG.md](RED_LOG.md)** — the round-trip PBT red-for-the-right-reason log.

## Run it

```
cargo test -p holon-toon                      # block-forest + generic (core) tests
cargo test -p holon-toon --features serde-json # + the nested-JSON adapter test
cargo run -p holon-toon --example measure --features measure -- <file.org> [more.org ...]
cargo run -p holon-toon --example measure --features measure -- synthetic  # JSON vs TOON
```

## TL;DR finding

- **Block forest** (fixed schema): TOON round-trips losslessly but saves only
  ~12% vs naive org and is **net-negative vs ID-compressed org** on real files —
  the irreducible cost is the per-block UUID (~45% of the payload), which no
  container format can dedup. **Recommendation: keep compressed org.**
- **Generic tabular** (the generalization): on wide, uniform query results TOON
  is a real win over JSON (repeated keys are the tax it removes). This is why
  the codec was generalized and made the MCP query tools' default output
  format rather than deleted (`format: "json"` opts out). See `RESULTS.md`
  for both tables.
