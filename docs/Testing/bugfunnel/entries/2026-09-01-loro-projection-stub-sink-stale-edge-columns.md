---
id: 2026-09-01-loro-projection-stub-sink-stale-edge-columns
date: 2026-09-01
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The Loro projection fault-injection test's in-memory sink hand-listed the
  junction-synthesized edge columns, so the `contributes_to` edge field left it
  behind and every recovery pass failed to decode its own rows.
---

## Bug
`holon-integration-tests::loro_suite
loro_projection_atomic_advance::failed_sink_write_neither_advances_base_nor_drops_change`
was RED on `main` itself (base `73a6c3e3`, and already red at `d49ef031`):

```
panicked at crates/holon-integration-tests/tests/loro_suite/loro_projection_atomic_advance.rs:291:32:
recovery pass succeeds: block block:B-id: required column 'contributes_to' absent from row
```

Found by the `loro-reds` lane re-running the suite by hand after the
2026-09-01 landing wave shipped with a named two-test allowlist in its loro
gate step. No gate had executed `loro_suite`, so nothing reported it.

## Root cause
`ToggleFailSink` stands in for both the projection's write bus and its
`SinkReader`. Its `read_blocks` rebuilt each row from the params
`block_to_params` wrote (which cover `block_raw` only) and then hand-listed the
junction-synthesized edge columns the real reader COALESCEs to `'[]'`:

```rust
for col in ["tags", "requires", "advice_suppressed"] { ... }
```

Production `TursoSinkReader::read_blocks`
(`crates/holon/src/storage/turso_sink_reader.rs:49-55`) COALESCEs **four**
columns — the fourth being `contributes_to` from the `block_contributes_to`
junction. When the Compass contribution edge field was added,
`Block::try_from` began requiring it (`crates/holon-api/src/block.rs:945`,
`require_string_array`, strict by design) and the stub's list went stale. The
failing pass is the recovery pass at step (d): the injected sink failure clears
`seeded`, so the next pass takes the full walk, calls `read_blocks`, and the
strict decode of the stored `B` row rejects the missing column.

`EdgeField::ALL` exists precisely to make this unrepresentable — its own
doc-comment says "Iterate this — never hand-list `tags`/`requires`"
(`crates/holon-api/src/edge_field.rs:37`) — and the stub violated it.

## Missing piece
Two things. (1) The stub mirrored a production SQL projection by hand instead
of iterating the closed `EdgeField::ALL` enumeration, so adding an edge field
could not propagate to it. (2) No `just` recipe or land gate ran
`--test loro_suite`, so the resulting red sat on `main` unnoticed and was
allowlisted rather than fixed.

## Remedy
FIXED. The stub now iterates `EdgeField::ALL` and COALESCEs `field.column()`,
so a fifth edge field cannot leave it behind. The escape route is closed at the
gate: `just loro-suite` was added AND wired into `just landing-gate` as an
explicit step (`landing [8/10]: loro consolidator suite`, justfile:1136-1137) —
the recipe existing was not enough, nothing called it. 2.2s of test time, ~24s
wall warm. `DEVELOPMENT.md`'s tier-L gate table lists it.

Teeth (production inversion, `lane-logs/red-teeth.1788252290.log`): making the
failed-sink arm of `LoroProjection::project`
(`crates/holon-loro/src/loro_sync_controller.rs:849-861`) apply its staging to
`live` anyway turns the restored test red for the right reason —
`staging is NOT applied on failure — 'live' must not gain C: ["block:C-id", "block:B-id"]`.
The inverted file was restored byte-for-byte (sha256
`a828609e097d5ab0f52e672b44dc2c421021d57be8000e545f232b1ec5d7cac5`).

Keystone repro: not applicable. The escape is in a harness stub, not a
production path the composed keystone drives; the keystone reads blocks through
the real `TursoSinkReader`, which was correct throughout.
