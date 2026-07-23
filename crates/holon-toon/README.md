# holon-toon (experiment)

A dense, round-trippable **TOON** ([toon-format](https://github.com/toon-format/toon))
projection of the Holon block forest — the representation an agent would read
when it queries a filtered block set and batch-patches it back. Built as a
structural sibling of [`holon-org-format`](../holon-org-format) so it can be
promoted directly if the experiment wins.

## Layout

| File | Role |
|---|---|
| `src/models.rs` | Block-forest domain types (`ToonBlock`, `BlockNode`, `Forest`) — parse-don't-validate newtypes mirroring `holon_api::Block` |
| `src/toon.rs` | Generic TOON primitive layer: scalar quoting/escaping, row split, props + list sub-codecs |
| `src/schema.rs` | The fixed 6-column tabular schema (`blocks[N]{id,depth,state,props,body,title}`) |
| `src/renderer.rs` | `Forest` → TOON |
| `src/parser.rs` | TOON → `Forest`, fail-loud (`Result` everywhere) |
| `src/org_reader.rs` | Minimal Org reader + two Org renderers, for the measurement only |
| `tests/roundtrip_proptest.rs` | The round-trip PBT (`parse(render(x)) == x`) |
| `tests/units.rs` | Golden render + fail-loud error paths |
| `examples/measure.rs` | Token measurement (org vs TOON) on real vault files |

## Docs

- **[MAPPING.md](MAPPING.md)** — per-construct org→TOON mapping & verdicts.
- **[RESULTS.md](RESULTS.md)** — token measurement table & recommendation.
- **[RED_LOG.md](RED_LOG.md)** — the round-trip PBT red-for-the-right-reason log.

## Run it

```
cargo test -p holon-toon
cargo run -p holon-toon --example measure --features measure -- <file.org> [more.org ...]
```

## TL;DR finding

TOON round-trips the Holon block forest losslessly, but saves only ~12% vs naive
org and is **net-negative vs ID-compressed org** on real files. The irreducible
cost is the per-block UUID (~45% of the payload), which no container format can
dedup. **Recommendation: keep compressed org; shorten ids instead.** See
`RESULTS.md`.
