---
id: 2026-08-24-declared-column-absent-narrow-subscription-projections
date: 2026-08-24
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  The eight chronic per-run "DECLARED column absent" warnings come from
  subscription SELECT lists that are narrower than the block entity's declared
  column set — NOT from CDC delta rows carrying only changed columns. Supersedes
  2026-07-19-boot-computed-field-profile-flood-fixed, whose stated 3-line
  terminal condition had drifted to 8 and whose proposed zero-warnings oracle
  was never built.
---

## Bug

Production logs eight chronic per-run WARN lines from
`warn_missing_declared_column` (`crates/holon-api/src/computed.rs:47`), each
naming a computed field whose required column is in the block entity's declared
schema but absent from the row that reached the enrich seat. Every one is a
real defect: the computed field is recorded as `Null` for that row, so the
render draws the field's default. The user-visible instance is a collapsed
block drawing the wrong bullet — `bullet_shape` needs `collapsed`, and without
it falls back to the plain `circle` instead of the ringed `orgmode` fisheye.

Found by production log review (this lane), following the deferral recorded in
the landed collapsed-bug rev `f7d04cd6`, which named the fix an architecture
fork "awaiting a ruling with the measurements".

This entry SUPERSEDES `2026-07-19-boot-computed-field-profile-flood-fixed`. That
entry fixed the original 869×/217× flood via type-aware binding and left two
things open: the `focus_roots` projection gap, and the boot-parity oracle. Its
stated terminal condition ("3 deduped LOUD lines") has since drifted to 8
unnoticed, precisely because the oracle it proposed was never built.

## Root cause

Two independent facts compose into the defect.

1. **`declared_columns` is entity-relative; the row is relation-relative.**
   `ProfileResolving::resolve_computed_only`
   (`crates/holon-profiles/src/lib.rs:1221`) selects the `EntityProfile` by the
   row id's SCHEME, and that profile's `declared_columns` is the entity's full
   persistent field set (`crates/holon-profiles/src/lib.rs:622`, from
   `TypeDefinition::persistent_fields()`). The row itself carries only the
   columns of the relation it was subscribed to. A subscription whose SELECT is
   narrower than the entity schema therefore trips the LOUD branch for every
   declared column it omits.

2. **Every enriched row comes from a matview, and enrichment is
   per-subscription.** `QueryEngine::watch_query`
   (`crates/holon/src/api/query_engine.rs:67`) compiles one query, opens a
   `RowChangeStream` for it, and wraps it in `enrich_stream`. So the column list
   a row arrives with is exactly that query's SELECT list.

MEASURED, not derived. `a_narrow_subscription_unbinds_the_declared_columns_it_omits`
(`crates/holon-integration-tests/tests/span_capture_suite/declared_column_parity.rs`)
subscribes `SELECT id, content FROM block_raw` through the production
`watch_query` seat and asserts SET EQUALITY against these eight
`(context, column)` pairs, held in its `EXPECTED_GAPS` const — one for one with
the production log. Set equality, not membership, because the drift this entry
documents (three lines becoming eight unnoticed) is exactly what a
count-tolerant assertion would let happen again: a lost pair and a new pair both
fail it. Log: `.lane-logs/g-parity-setpin.log`. This is the causal proof that
the warnings come from the SELECT list and from nothing on the CDC path.

The eight warnings are the complete cross-product of the block profile's
computed fields (`assets/default/types/block_profile.yaml`) against the four
declared columns a narrow projection drops:

| # | computed field | required declared column |
|---|---|---|
| 1 | `is_source`       | `content_type`    |
| 2 | `is_image`        | `content_type`    |
| 3 | `is_holon_source` | `source_language` |
| 4 | `is_rule_head`    | `source_language` |
| 5 | `is_legacy_rule`  | `source_language` |
| 6 | `bullet_shape`    | `collapsed`       |
| 7 | `is_widget_only`  | `widget_only`     |
| 8 | `is_program`      | `parent_id`       |

`collapsed` and `widget_only` are plain persistent `bool` fields on `Block`
(`crates/holon-api/src/block.rs:370,375`), so they ARE in `declared_columns`;
the `#[edge_field]` columns (`tags`, `requires`, …) are excluded by the derive
and stay silent, which is why `is_page_row` does not appear above.

Narrowing SELECTs identified so far, all of which drop `collapsed` and
`widget_only`:

- `crates/holon/sql/prql_stdlib.prql` — `descendants`, whose explicit `select`
  lists eleven columns and omits `collapsed`/`widget_only`. This is the query
  behind the `embedded_page` / `embedded_page_expanded` variants'
  `live_query(#{prql: "from descendants"})`, i.e. the main outline.
- `crates/holon/sql/prql_stdlib.prql` — `focused_children`, same omission.
- `crates/holon/sql/startup/preload_blocks.prql` — `select {id, parent_id,
  content, content_type, source_language}`.
- `crates/holon/sql/startup/preload_text_blocks.prql` — `select {id, content}`,
  which drops all five columns above.

The remedy pattern is already ratified in-tree for exactly this failure: the
`backlinks` matview projects EVERY block column
(`crates/holon-turso/src/schema_modules.rs:1134`) and is guarded by
`backlinks_view_projects_every_block_column` (`:1328`), whose comment names the
same computed fields and the same columns.

**The brief's premise is REFUTED.** The rows are not CDC deltas carrying only
changed columns. Holon never enables CDC v1 anywhere (no
`capture_data_changes` / CDC pragma exists in `crates/`; the production
connection is opened by `create_connection_internal`,
`crates/holon-turso/src/turso.rs:2150`, which sets only foreign keys and a busy
timeout). Consistent with the fork session's measurement on this pin, the
consumer receives full row images: `process_cdc_event`
(`crates/holon-turso/src/turso.rs:2286`) zips `parse_record()` against
`event.columns`, and `coalesce_row_changes` (`:1647`) folds the matview
Delete+Insert pair into an `Updated` carrying the INSERT's full image. There is
no narrowing anywhere on the CDC path; the narrowing is in the view/query
projection, which is a deliberate authored SELECT.

## Missing piece

The zero-warnings oracle proposed by the 2026-07-19 entry and never built. The
signal is WARN-level, and `inv-no-observed-errors` keys on ERROR, so the whole
class sits below every existing invariant threshold — which is how the terminal
condition drifted from 3 to 8 with nothing going red. This is an ORACLE gap:
the keystone can generate the interaction (it renders blocks through the same
profiles), but no invariant would flag it.

Secondary ENVIRONMENT: the specific narrow SELECTs live in the shipped query
pack and startup preloads, and the keystone's own topology does not subscribe
through all of them.

## Remedy

OPEN.

Built here: the parity oracle,
`crates/holon-integration-tests/tests/span_capture_suite/declared_column_parity.rs`
— boots a production-shaped `TestEnvironment` over a seeded vault, drives a
representative write set, and asserts ZERO `DECLARED column absent` lines. It
needs both halves of the capture contract: `SpanCollector::global()` before the
SUT boots, and `holon_api::computed::reset_missing_declared_warnings()` (added
here) before it, because the once-per-`(context, column)` dedup is
process-global and would otherwise suppress the very signal under assertion.

A SECOND ORACLE GAP, inside the oracle. The boot-shaped parity test came back
GREEN on the unfixed tree, with its vacuity guard passing (CDC batches did reach
the enrich seat). The headless `TestEnvironment` boot exercises the enrich seat
but never subscribes through the narrow production queries: `focused_children`
needs a navigation cursor and `descendants` needs an embedded page to render,
and a boot-plus-`set_field` write set produces neither. A zero-warning result
there is therefore not evidence that production is clean.

It has been demoted accordingly rather than left to read as a parity pass —
renamed `default_boot_and_edit_path_carries_its_declared_columns`, with its
scope and its measured-green-on-an-unfixed-tree status stated in its own doc
comment. It stays as a tripwire for that one path; it is not the guarantee.

The arithmetic also refuses to close on the narrow SELECTs found so far:
`descendants` + `focused_children` drop `collapsed` + `widget_only`, which is
two of the eight. Production shows all eight, so either the eight is the dedup
UNION across several narrow subscriptions (the dedup is process-global and never
reset in production, which makes this likely) or a narrower subscription remains
unfound. `preload_startup_views` (`crates/holon/src/di/lifecycle.rs:27`) is
EXONERATED: it only creates the matviews and never subscribes, so the startup
preloads never flow through enrichment — they need neither widening nor an
enrichment exemption. Attributing the remaining six is a precondition of the
fix, not a detail to wave through.

The right home is therefore a COMPOSED KEYSTONE INVARIANT
(`inv-no-declared-column-absent`), BUILT and wired, engaged 11/12 transitions in
keystone-smoke. It runs OBSERVE-ONLY (the `HOLON_PBT_RESEED_ORACLE` precedent): unset it reports
`InvariantResult::Skipped` and reddens nobody; with
`HOLON_PBT_DECLARED_COLUMN_ORACLE=enforce` it fails naming all eight pairs.

Both directions are proven on the HAND-AUTHORED gate, whose replays are fixed:
observe `9 passed` with 70 `declared_column_oracle` WARN lines
(`.lane-logs/g3-hand-observe.log`), enforce `[inv-no-declared-column-absent] 8
projection gap` (`.lane-logs/g3-hand-enforce.log`).

Use that gate, not the smoke, to judge this oracle. `keystone-smoke` runs one
drawn case and routinely draws a case that touches no narrow subscription — an
enforce-mode smoke run has been observed GREEN on a tree with all eight gaps
live. So a green smoke is not evidence the gaps are gone, and whoever flips D7
should expect the deterministic signal in hand-authored.

Observe-only mode also EMITS what it sees, as a `declared_column_oracle` WARN
naming the pairs. Without that the observation would reach nobody: no consumer
reads a `Skipped` payload — the harness tallies the disposition and drops the
string, and `first_divergent` renders it "skipped (observed nothing)", the exact
inverse of a gap-carrying Skip. Note also that the `n/m` counter beside the
invariant in run output reports SELECTION, not observations. The cap yields the
extracted `field/column` pair rather than the raw warning text on purpose: the
emitted WARN is itself captured, and a payload repeating the signal verbatim
would be re-read as a gap next transition (the self-feeding escape loop
`test_tracing` records from 2026-07-11).

It is held there deliberately, because enforcing today would redden the keystone
on ITS OWN generated projections: `crate::pbt::query::all_block_columns`
(`crates/holon-integration-tests/src/pbt/query.rs:822`) is
`{id, content, content_type, source_language, source_name, parent_id}` — no
`collapsed`, no `widget_only` — so every `TestQuery::layout` trips it, and
narrower subsets are generated on purpose. Widening the production SELECTs would
therefore NOT turn it green, and editing the generator to suit the oracle would
mask the class.

That exposes the real fork, now decision card D7: this invariant asserts narrow
projections are bugs AS A CLASS, but `assets/default/types/block_profile.yaml`
documents an absent `collapsed` as SAFE degradation ("the `icon` name falls back
to its 'circle' default — a missing column degrades safely to the plain dot").
Either that comment is wrong and the column is mandatory, or the warning
over-reports a supported case; both cannot hold, and which one gives decides
whether this is one bug or eight false positives. The likely resolution is
narrower than either: require the column where the RENDER binds it — the outline
must carry `collapsed` because it draws disclosure state, while a user's narrow
`live_query` whose render never binds the field degrades legitimately.

The invariant is built on the `observed_errors.rs` pattern —
a `capmap_adapter` cap reading `SpanCollector::captured_warnings()` filtered on
this signal, with `reset_missing_declared_warnings()` wired into the same
per-transition metrics lifecycle that resets the problem window. The keystone's
`NavigateFocus` / `CreateBlockUnderFocus` transitions DO exercise the outline
subscriptions the headless boot misses. That is the ORACLE remedy this gap
class calls for, and it is the prerequisite for verifying any widening.

Fixed here (adjacent fail-loud violation, independent of the above):
`process_cdc_event` no longer silently `continue`s when `parse_record()`
returns `None`. Each failure is named at ERROR with relation, rowid, change
kind, and schema width, and the batch's `BatchMetadata.degraded` carries the
disclosure so the reactive watcher surfaces it on the render. A batch whose
changes ALL failed to parse is now still broadcast, since that is exactly when
consumers most need telling.

The projection widening itself is ESCALATED rather than applied: widening
`descendants` / `focused_children` changes the shape of the IVM-maintained
matviews behind the main outline, which carries latency-SLO and engine-gap
consequences that want a ruling. The measurement above is what that ruling was
waiting for, and it collapses the three-way fork the collapsed-bug lane
recorded — "hydrate vs no-compute-on-delta vs declared-relative-to-source" all
presuppose a CDC-delta narrowing that does not exist. The remaining choice is
the already-ratified one: widen the authored SELECTs, keep the warning as a
true positive, and let the oracle hold the line.
