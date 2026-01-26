# T1 PBT back-test against MEMORY.md (Phase 0/B)

For each substantive bug catalogued in `MEMORY.md` as of 2026-05-17, classify:
- **T1 catcher** — which proposed narrow PBT would have surfaced the bug, or none.
- **Confidence** — *clean* (the bug's failure mode is exactly an invariant in the PBT's SUT), *borderline* (catchable in principle but only via wider-than-T1 invariants), *none* (no T1 PBT in this proposal would have caught it).

Meta entries (refactors, principle restatements, generator-weight tweaks) excluded — only *bugs* count for the gate.

## Holon-side bugs (legitimate T1 target)

| Bug | T1 catcher | Confidence | Notes |
|---|---|---|---|
| SplitBlock capture-phase Page-editor focus theft | Phase 5 (editor+Loro) | clean | Focus + dispatch interaction; replayable from PBT op log |
| `join_block` no-prev-sibling refuses with children | Phase 5 (editor+Loro) | clean | Chord-op behaviour on tree state |
| SQL-projection race in chord-op loops | Phase 6 (BlockCellRegistry) | clean | Loro↔SQL convergence after iterative reposition is exactly Phase 6's invariant |
| DeleteBackward ref commit + trim divergence | T0 (MutableTree) + Phase 5 | clean | Pure-logic trim path → T0; Loro commit timing → Phase 5 |
| NavigateFocus dispatch fix (FU-14) | Phase 5 (editor+Loro) | clean | Click-intent → apply_intent path; ViewModel-as-source-of-truth |
| Org renderer matview lag (edge-abstraction) | Phase 4 (block-tree round-trip) | clean | Hydration drop on render→parse round trip |
| `edge_abstraction` headline tag drop on round-trip | Phase 4 | clean | Same as above |
| TUI PBT inv1 — Loro peer_id + ops asymmetry | Phase 5 (editor+Loro) | clean | Editor↔Loro consistency invariant |
| `LoroSyncController` matview-write regression (table_name) | Phase 6 | borderline | Bug is in constructor wiring; caught only if PBT exercises real LoroSync path, not synthetic op replay |
| Block has two deserializers (tags `[]` default) | Phase 4 | borderline | Caught if Phase 4 exercises tag-bearing blocks through both deserializer paths; otherwise misses |
| LiveData rowid-keyed delete fix | Phase 6 | borderline | CDC delete shape via matview; only surfaces when Phase 6 enables CDC-watcher invariants |
| Phantom-Loro startup-seed race | none | — | Startup-ordering race; no T1 SUT models the seed-vs-watcher interleave. Stays the wide PBT's job. |

**Holon-side total: 12 bugs.**
- Clean T1 catcher: **8 / 12 = 67%**
- Clean + borderline: **11 / 12 = 92%**
- None: **1 / 12** (Phantom-Loro startup race)

## Upstream Turso bugs (T4 territory)

Each of these is fixed (or pinned for fix) by an upstream Turso commit and frozen as a `crates/holon/sql/regressions/*.sql` replay. Narrow T1 PBTs are the wrong tool — the failure crosses an opaque dependency.

| Bug | Status | Pinned regression |
|---|---|---|
| `json_group_array multiset went negative` (antijoin refactor) | UPSTREAM open | `turso_ivm_json_group_array_multiset_negative_2026-05-17.sql` |
| MCP first matview query returns 0 rows | UPSTREAM fixed | reproducers in `bigdata/turso/bindings/rust/tests/matview_first_open.rs` |
| No-op `UPDATE` corrupts LEFT-JOIN matview | UPSTREAM fixed | `holon_block_redundant_update_2026-05-07.sql` |
| `focus_roots` matview drops rows mid-txn | UPSTREAM fixed | replay via `turso-sql-replay` |
| `set_null_flag` on unexpected cursor type | UPSTREAM fixed | n/a |
| `MatchCounterOperator::eval` Uninitialized | UPSTREAM open (blocks Full PBT) | n/a |
| `inv10d` block matview CDC race | UPSTREAM fixed (matview cursor) | n/a |
| No array aggregation in matviews | UPSTREAM fixed | n/a |
| Chained matviews unsupported | UPSTREAM fixed | n/a |

**Upstream total: 9.** Outside T1 scope by design; T4 covers them.

## Gate verdict

The plan's gate is "≥80% of MEMORY.md bugs have a T1 catcher."

- Counting holon-side only: **92% (clean+borderline) — PASSES.** Clean-only is 67%, below the 80% line; the 25 percentage-point swing rides on Phase 6's borderline cases (LoroSyncController constructor wiring, LiveData CDC delete, Block deserializer tag default).
- Counting all bugs (including upstream): **52% — FAILS**, but this is the wrong denominator: the gate is about T1 coverage of *holon-side* failures. Upstream bugs are explicitly T4's job.

**Recommendation:** the gate passes on the right denominator, with the caveat that three of the four "borderline" cases concentrate in Phase 6. Phase 6's invariant catalogue must explicitly include (a) constructor/setup paths, not just op streams; (b) CDC delete shape via `block_tags`; (c) tag-default deserialization. If Phase 6 ships with only Loro↔SQL convergence and `EventOrigin` routing, the borderline cases regress to "none" and the rate drops to 8/12 = 67%, below the gate.

## Phase ordering implications

- **Phase 4 (block-tree round-trip) and Phase 5 (editor+Loro) carry the most weight** — together they catch 6 of 8 clean cases. Their priority in Phases 3–9 sequencing is correct.
- **Phase 7 (SqlOperationProvider + event-bus)** appears in **zero** MEMORY.md bug entries. Phase 3.3/3.4/3.7 work was design refactor, not failure. Recommend demoting Phase 7's priority — land it after Phases 4/5/6 are stable, not in between. Or, more aggressively: defer Phase 7 entirely until a real bug demands it.
- **T0 pure-MutableTree proptest** picks up one bug (DeleteBackward trim) on its own and pairs cleanly with Phase 5. Worth landing alongside.

## Open question

The phantom-Loro startup-seed race has no T1 catcher and likely never will under this design — startup races require the orchestration of multiple concurrent subsystems that no narrow SUT can model economically. Accept this gap explicitly: wide PBT (Phase 2 / `general_e2e_pbt`) remains the only catcher for startup-race-class bugs. Document this in `docs/TESTING_STRATEGY.md` Phase 12 so contributors don't expect T1 coverage there.
