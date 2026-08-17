# Increment narratives with no confidently-matched ledger row

Each was written as a second record of a bug the ledger
already counts. Attach the prose to the named entry, or record
why no entry exists, then delete the line.

## legacy line 154 (ENVIRONMENT 2026-08-04)

dogfood on a COPY of the real vault, port 8730 — two org files declaring the
SAME `#+ID:` are silently merged into one page block instead of failing loud
at ingest; on the real vault `Projects/Agentic DPL.org` and
`Projects/DBG/Agentic DPL.org` both carry `#+ID: 9464fbf0-…`, their two
distinct child directories collapse under one parent, and write-back for one
of the resulting same-named `Prototype` siblings is QUARANTINED for the rest
of the session — user edits to that page never reach disk, disclosed only in
the log; the quarantine lifts on the next fully-successful ingest, which
never happened here)

## legacy line 156 (ENVIRONMENT 2026-08-04)

dogfood, real-vault scale — cold boot ingest of a 182 MB / 139-file vault
takes `boot_ingest_total` 237 s, with `boot_file` p95 7.36 s and max 28.5 s
for a single file; no automated layer boots at this scale, so the whole
cold-start cost is unmeasured)

## legacy line 157 (COVERAGE 2026-08-04)

dogfood, left sidebar — a Page-tagged block with empty content renders as a
completely BLANK sidebar row; the disclosed `(untitled)` placeholder can
never appear because `empty` is not a declared param of the `text`
widget_builder, so the kwarg is silently dropped before reaching the
renderer)

## legacy line 158 (ORACLE 2026-08-04)

dogfood, template feature — undoing a template instantiation left a 19/20
partial instance: after instantiating a 20-child template one `undo` removed
exactly ONE child and the stack then reported "Nothing to undo", with no
route back to either the clean or the complete state; root cause UNRESOLVED
— per-write undo granularity vs. origin/session-scoped undo filtering of the
MCP-issued writes — needs a re-drive before any fix)

## legacy line 159 (ORACLE 2026-08-04)

dogfood — the live `latency-slo` oracle false-positives on a backgrounded
window: two `navigate` interactions were reported at 150 s and 161 s
end-to-end because GPUI does not paint while not frontmost, so the visible
stage only lands when the window is re-fronted; the red ORACLE VIOLATION
banner is indistinguishable from a genuine SLO breach)

## legacy line 166 (ORACLE 2026-08-03)

agent exploration, geometry-precision lane's live `describe_ui` dump —
`BoundsTracker`, a pure-observability wrapper, MUTATES production layout:
`request_layout` forces `width: relative(1.0)` + `flex_grow: 1.0` on a
`Style::default()` (`display: Block`), so the two `tracked()` siblings of
the shipped block row (`selectable` and `rendered_text`) each demand the
full row width, flex-shrink splits them 420/420 of 844, the bullet's
click/drag region claims the left half, the text is displaced to x=780, and
the block wrapper collapses to height 0 — the exact failure mode
`live_block.rs:33-37` already documents and works around at ONE of five call
sites; the `expected-size-satisfied` oracle that would catch it is unarmed
because `with_expected_size` has zero production call sites)

## legacy line 167 (ENVIRONMENT 2026-08-03)

dogfood I6, chat-input gate — the shipped `send_message` wiring can never
deliver: Holon dispatches `{message, id: <full session uuid>}` while the
real `claude-code-history-mcp` tool requires `{text, id: <background SHORT
id>}`, so every approved send dies at the provider with `text is required
and must be a string`; the mock encodes Holon's assumed names, so no
automated layer ever compared them to the real tool schema) — FIXED same
day; the generic sidecar-vs-`tools/list` contract check remains open

## legacy line 169 (PERCEPTION 2026-08-03)

dogfood I6, chat-input gate — `describe_ui` reports an EXPANDED
`expand_toggle` as collapsed (`▶`, `content=UNEVALUATED`) while the window
paints its whole content subtree, and its sidebar `live_query` marker states
a reason that is factually false; the only agent-facing inspection tool
contradicts the painted UI, so the chat view cannot be verified headlessly)

## legacy line 171 (COVERAGE 2026-08-02)

agent exploration — `shift_action`/`cmd_action`/`ctrl_action`/`alt_action`
are absent from the global `is_template_arg` allowlist
(`crates/holon-api/src/render_eval.rs:681-696`), so `selectable`'s
`get_template` returns `None` for all four and EVERY modifier-click action
is dead: cmd/ctrl-click "open in tab" in the shipped left sidebar and
shift-click `focus_pin` in all three `block_profile.yaml` render strings; no
harness anywhere issues a CLICK carrying a modifier)

## legacy line 172 (COVERAGE 2026-08-02)

dogfood, ClaudeCode.org page build-out, RE-TRIAGED after adversarial
verification — the shipped left-sidebar Integrations section renders NOTHING
even when `sync_states` has rows: `live_query`'s GPUI builder forces
`height: relative(1.0)` under `ShellPlacement::Panel`
(`frontends/gpui/src/render/builders/live_query.rs:69-77`) and collapses to
zero against a non-definite parent `column`; no test anywhere renders a
live_query's ROWS)

## legacy line 173 (PERCEPTION 2026-08-02)

dogfood, ClaudeCode.org page build-out — `describe_ui` builds live_query
content with `with_data_rows(vec![])`
(`crates/holon-frontend/src/render_interpreter.rs:655`), so it emits one
empty-column placeholder for a WORKING live_query; the only agent-facing
UI-inspection tool cannot see live_query rows and misdiagnosed this very bug
as a row-binding failure)

## legacy line 174 (PERCEPTION 2026-08-02)

dogfood, ClaudeCode.org page build-out — an unparseable render_source
silently falls back to table(), so a discarded render looks like a working
page)

## legacy line 175 (COVERAGE 2026-08-02)

dogfood, ClaudeCode.org page build-out — the render DSL cannot express a
two-level drill-down in a collection (Rhai ExprTooDeep) and fails silently)

## legacy line 176 (COVERAGE 2026-08-02)

dogfood, ClaudeCode.org page build-out — widget calls are registered only
for arities 0..=6, so a 7-child row() fails to parse)

## legacy line 177 (ORACLE 2026-08-02)

dogfood, ClaudeCode.org page build-out — a source block's sort/take are
dropped by the rendered collection while filter is honoured)

## legacy line 178 (ENVIRONMENT 2026-08-02)

dogfood, ClaudeCode.org page build-out — editing a source block's query text
leaves its rendered collection stale, then permanently empty, until restart)

## legacy line 179 (PERCEPTION 2026-08-02)

dogfood, ClaudeCode.org page build-out — chat_bubble hardcodes a dark
palette and an "H" avatar instead of reading the theme)

## legacy line 180 (ENVIRONMENT 2026-08-02)

dogfood, ClaudeCode.org page build-out — the GPUI container provides no
DegradedSignalBus, so integration connect failures render blank with no
banner)

## legacy line 181 (ORACLE 2026-08-02)

dogfood, ClaudeCode.org page build-out — cold boot on the real vault spends
8m22s in the org scan, holding the integration sync gate closed the whole
time)

## legacy line 182 (ENVIRONMENT 2026-08-02)

dogfood, ClaudeCode.org page build-out — click{entity_id} hangs forever on a
non-block entity and x/y never hits an expand_toggle chevron, so drill-downs
are undriveable)

## legacy line 183 (COVERAGE 2026-08-02)

dogfood, ClaudeCode.org page build-out, ROOT CAUSE CORRECTED —
`expand_toggle` content NEVER renders in any state because
`get_template("content")` asks for a name absent from the `is_template_arg`
allowlist (`render_eval.rs:681-696`); the allowlist is keyed globally by
name while templateness is per-widget, which also kills cmd/ctrl-click in
the shipped sidebar)

## legacy line 184 (COVERAGE 2026-08-02)

agent exploration — `resolve_image_path` guard is a no-op for relative
traversal (`Path::starts_with` is component-wise, so
`joined.starts_with(root)` is true for `<root>/../evil.png`), and the
function returns the UN-normalized `joined` it never checked, so
`materialize_images` `create_dir_all`+`write`s image bytes OUTSIDE the vault
from CRDT-synced `block.content`; the image-block generator only ever emits
`attachments/<stem>.<ext>`)

## legacy line 189 (COVERAGE 2026-08-02)

agent exploration — `.with_extension("org")` REPLACES the trailing dotted
segment of a page title, so `citrix-STX.BROWSER_AGENT` derives
`citrix-STX.org`: a title round-trip identity break and a filename collision
between two differently-titled pages; every title the generators can draw is
dotless)

## legacy line 193 (ENVIRONMENT 2026-08-02)

agent exploration — `build-release-aab.sh` links with no compiled resources
while the shared manifest references `@mipmap/ic_launcher`; no automated
layer runs any Android packaging path, so the defect is invisible until a
release is cut)

## legacy line 196 (COVERAGE 2026-08-02)

dogfood — a vault SUBDIRECTORY with no sibling `<dir>.org` companion
(`Projects/Aiuno/`, `Agents/citrix/`) leaves a permanent EMPTY placeholder
`Page` root in the tree: titleless sidebar rows, plus a name-chain path
escape that wrote `holon-pkm.org` OUTSIDE the vault and disabled write-back
for two real pages; every generated filename is flat)

## legacy line 200 (COVERAGE 2026-08-02)

dogfood — three unclearable "Shared edit saved — org file pending" toasts
raised for 111 ordinary vault blocks carrying a STALE `:shared-tree-id:`
drawer property whose mount is not a page; the mount generator is gated off
by `HOLON_PBT_SHARED_TREE_MOUNT`)

## legacy line 203 (COVERAGE 2026-08-01)

task #14 — a link target whose path starts with a colon (`[[tag::x]]`) is
rewritten to `[[block:tag::x][tag::x]]` on write-back, and `[[tag:a b]]`
panics the ingest; no generator draws a colon-leading-path target)

## legacy line 206 (COVERAGE 2026-08-01)

task #98 — entity-scheme registration races the boot org scan, so a
sidecar-declared `[[<entity>:<id>]]` link ingested before the provider
connects is permanently unresolved; no transition can register an entity
type at runtime)

## legacy line 246 (COVERAGE 2026-08-01)

a watched query whose trailing `ORDER BY` qualifies its columns with a
source table alias (`SELECT b.* FROM block b JOIN block_tags bt … ORDER BY
b.sort_key` — the SHIPPED sidebar shape, and what GQL compiles the
right-sidebar query to) failed the view read with `no such table: b`.
`MatviewManager::watch` (`crates/holon-turso/src/matview_manager.rs:807`)
lifts the clause off the SOURCE query via `util::trailing_order_by` and
`query_view_ordered` (`matview_manager.rs:765`) splices it VERBATIM onto
`SELECT *, rowid AS _rowid FROM watch_view_… {clause}`, where the source
`FROM` aliases are out of scope. Second, quieter manifestation at the same
clause: `util::order_by_sort_spec` rejected the dot in `b.title` as an
inexpressible expression, so `BackendEngine::query_ordering_spec`
(`crates/holon/src/api/backend_engine.rs:327`) returned `None` and the
collection rendered in default order behind a warn-level trace only. The
keystone HAS the seam (`SetupWatch` → `register_watch` with caller-supplied
SQL, reaching `query_ordering_spec`) but `TestQuery::to_sql/to_prql/to_gql`
never emits an `ORDER BY` at all, so the triggering interaction is
ungeneratable — the alphabet is narrowed, which is #42's blocked
`QueryTable` widening. Stood in with a dedicated integration test against
real Turso (`crates/holon-turso/tests/watch_preserves_order.rs`,
`watch_honours_an_alias_qualified_order_by` +
`sort_spec_sees_through_a_table_alias`), both red-for-the-right-reason first
(`no such table: d`; `None` vs `Some("title")`). FIXED: new
`util::rewrite_order_by_for_view` re-expresses the clause in the view's OWN
output columns — dropping the qualifier is SQLite's own view-column naming
rule for an unaliased `t.col` projection — verified against `PRAGMA
table_info(<matview>)` (supported for matviews in our turso fork,
`core/translate/pragma.rs:1317`) and `bail!`ing with the term, the derived
column and the view's real column list when the projection renamed it. No
silent drop: that is the #72 class. NOT fixed here and NOT the same defect:
#72's `LIMIT` drop is the same seam's other arm — `strip_order_by` removes
ORDER BY/LIMIT/OFFSET together for the matview body while
`trailing_order_by` deliberately re-emits only the ORDER BY, documented at
`util.rs:46-48`, because the matview holds the unbounded relation and its
CDC stream delivers changes beyond any window.)

## legacy line 274 (COVERAGE 2026-08-01)

no secondary: `format_properties_drawer` alphabetized every `:PROPERTIES:`
drawer on write-back while the file on disk holds the author's insertion
order, so ingest→write-back moved 3 lines of a real vault file for no
semantic gain. Test-adjacent discovery: found by task #67's own
byte-stability acceptance run, which had to allowlist the file rather than
by any assertion going red on the defect itself. The gap is the START STATE
of every org round-trip PBT — they begin from a synthesized `Block` and
assert render→parse→render is a fixed point, which a NORMALIZING renderer
satisfies by construction because its own first render establishes the order
the second reproduces. Only a disk-first property can see it.)

## legacy line 283 (ENVIRONMENT 2026-08-01)

secondary ORACLE: the P0 write-back-quarantine guard
`region_writeback_loss::partial_ingest_does_not_rewrite_the_file`
(`crates/holon-integration-tests/tests/region_writeback_loss.rs:232`) has
been RED on main since 2026-07-27 and STOPPED TESTING ITS OWN SUBJECT. Not a
prod defect: nothing is lost. The test stages a "partial ingest" by
authoring `:ID: shared-child` into `zzz_bad.org` while `aaa_owner.org`
already owns that id, expecting the cross-document re-parent to fail
mid-scan (Loro `resolve_parent_tree_id`) so `on_file_changed`'s `Err` arm
quarantines the file
(`crates/holon-filesystem/src/file_sync_controller.rs:1513`) and disk stays
byte-identical. Since `cc73054f6a77` ("fix(sync): cross-doc ingest guard +
stale-block writeback prune") the cross-doc-membership guard
(`file_sync_controller.rs:2561-2593`) intercepts FIRST: `shared-child` is
folded into `foreign_subtree_ids`, no Move is ever authored, ingest returns
`Ok`, and the sanctioned prune plus the `needs_id_writeback` forced
round-trip (`file_sync_controller.rs:3356`, file has no `#+ID:`) rewrite the
file. Measured on main: ZERO `ingest FAILED partway` events, ONE `cross-doc
membership` warn, and the on-disk delta is exactly (a) a `#+ID:` header
gained and (b) `*** Shared`/`shared-child` pruned — `bad-top`, `Parent`,
`bad-tail` and every body line survive. ENVIRONMENT: the fixture's stand-in
no longer reaches the code path the test exists to pin, so the quarantine
branch is now UNEXERCISED by this file. Secondary ORACLE: the assertion is
byte-equality over the whole file, which judges "the file did not change"
rather than the contract "no un-ingested line was lost", so it cannot
distinguish a sanctioned convergence from real loss and it went red for a
reason it was never about. Attribution pinned by A/B in one workspace: GREEN
at `70226735aa8e` (1 passed), RED at `cc73054f6a77` and RED on main
`cf6702487db1` with a BYTE-IDENTICAL failure signature — so the recent org
render-ladder work (`3508205e50`, #67) is EXONERATED. NOT FIXED (triage-only
pass, task #95): the repair is to restage a genuine partial ingest that the
cross-doc guard does not absorb and to assert survival of the authored ids
rather than byte-equality; until then the quarantine branch's only remaining
coverage is the sibling `three_mode_region_survives_ingest_and_writeback`,
which passes.)

## legacy line 309 (ENVIRONMENT 2026-07-31)

secondary COVERAGE: the keystone harness's org serializer
(`serialize_block_recursive`,
`crates/holon-integration-tests/src/org_utils.rs`) RE-IMPLEMENTED
block-content emission instead of calling prod's. It projected marks with
the INNER `render_inline_marks` and otherwise wrote `block.content` raw,
while prod's write-back (`WritebackRenderer::render_blocks` →
`OrgRenderer::render_walk` → `Block::to_org` → `render_headline_block` →
`render_block_content`) goes through the checked degradation ladder, which
also verbatim-quotes markup-shaped literals. A block whose content is
`__default__` with no marks therefore reached disk as `=__default__=` from
prod and as `__default__` from the harness — and the harness form does not
round-trip (it re-parses as `default` + Underline marks), so any transition
that rewrote a whole doc through the harness silently corrupted content the
SUT held correctly. ENVIRONMENT: the harness ran a DIFFERENT renderer than
prod; every ladder change landed the divergence again. Secondary COVERAGE:
generated content is pre-normalized to the org round-trip fixed point
(`normalize_content_for_org_roundtrip`), so a store block holding a
markup-shaped literal with NO marks is not currently generatable — which is
why no case ever hit it. Found by review, not by a test. FIXED: the
serializer now calls `holon_orgmode::models::render_block_content` — the
same entry point prod's write-back uses; pinned by
`crates/holon-integration-tests/tests/org_serializer_prod_content_parity.rs`,
which asserts headline parity with prod AND content round-trip.)

## legacy line 344 (ORACLE 2026-07-31)

secondary ENVIRONMENT: `test_platform_geometry_determinism::
test_platform_geometry_is_real_and_deterministic` is FLAKY on unmodified
main — measured 2 failures in 6 consecutive runs with no source change.
Always the same signature: boot 0 records 99 elements against 92 on boots 1
and 2, and all 7 extras are ZERO-HEIGHT chrome of one sidebar-shaped tree
row (`icon`, `row`, 3×`spacer`, 2×`text` at y=137–150, x=12–150). ORACLE:
the test's own stable signal is invariant across every run, failing or
passing — 80 non-degenerate elements, 8 distinct entities — but the
assertion compares the FULL element multiset including in-flight zero-height
records, so it judges a transient the app is entitled to have rather than
the geometry contract it exists to pin. Secondary ENVIRONMENT: which boot
captures the transient depends on boot timing (the failing runs also log
`[GPUI] pre-warm timeout`). Fix direction: compare the non-degenerate
geometry the test already computes, or settle the frame before snapshotting;
do NOT simply retry, which would hide a real determinism regression. Found
while gating the #69 fix: an A/B of 6 runs per side gave 2/6 failures
WITHOUT the fix and 4/6 WITH it. The failure is therefore NOT attributable
to #69 — it reproduces on untouched code — but note honestly that n=6 per
side cannot distinguish those rates (Fisher p≈0.57; p≈0.18 pooling earlier
observations), so whether the #69 fix raises the rate is UNRESOLVED and
would need ~30 runs per side to settle.)

## legacy line 360 (ORACLE 2026-07-31)

a `[tree-desync]` ERROR storm — 300+ events per page-navigation click, in
both directions ("in provider but not row_map", "in row_map but not
provider" naming the PREVIOUS page's rows). ORACLE, not a data defect: the
probe in `crates/holon-frontend/src/reactive_view.rs`
(`reactive::tree_desync`) compares provider ↔ row_map ↔ tree inside
`apply_diff`, i.e. after EVERY individual `VecDiff`, while the contract it
encodes — row_map == provider — is a CONVERGENCE contract that may only be
judged after a delta batch has settled. Draining the previous page arrives
as one `RemoveAt` per row, so the probe necessarily fires on every
intermediate state. Evidence (`/tmp/dogfood-0731-evidence/logs/app.log`,
per-line divergence sizes across one burst): the row_map-side excess falls
monotonically 13, 12, 11 … 2, 1 and then stops — the final delta reaches
equality and logs nothing. The state CONVERGES; only the evaluation point is
wrong. Investigated as the suspected cause of the #69 band-geometry bug and
found SEPARABLE: this divergence is about the outline's row set during
navigation, #69 is about the height reserved for one row of a settled page.
NOT yet fixed — the fix is to evaluate the probe at a settle boundary (once
per frame, in `render`) rather than per delta, keeping it fail-loud there;
silencing it is explicitly not the fix, because a divergence that SURVIVES
the batch is a real bug this probe exists to catch.)

## legacy line 376 (PERCEPTION 2026-07-31)

secondary ORACLE: after the #60 nested-shell fix a nested query band PAINTS
its rows, but the outline reserves far less height than the band paints —
following sibling rows draw ON TOP of the band's lower rows (Martin's
ClaudeCode page rendered all five sections overlapping each other; evidence
shots 01/03/04 in `/tmp/dogfood-0731-evidence/shots/`) and the page's scroll
extent falls short, leaving rows past it permanently unreachable.
Differential control: the same row count as PLAIN outline blocks scrolls
fine. PERCEPTION for the same reason as #60's row directly below — every
model-layer oracle is satisfied (the ViewModel holds the rows, the query
returns them, the rows are even painted now); what is wrong is a
RELATIONSHIP between painted boxes, and nothing in the suite compared one
row's bounds against another's or checked that the scroll extent covers the
content. Secondary ORACLE: `BoundsRegistry` already carries x/y/w/h, so both
invariants were expressible and simply did not exist. Covered by
`frontends/gpui/tests/gpui_window_slice.rs::band_rows_do_not_overlap_the_following_sibling_row`
and `::page_with_a_nested_band_scrolls_to_its_last_row`, plus the
fixture-tier control `frontends/gpui/tests/nested_band_height_spike.rs`. The
control is GREEN, which REFUTES the first hypothesis — a stale gpui
`ListState` measurement for the band's row — at the fixture tier: with a
demonstrably virtualized outline, gpui re-measures a visible row whose
nested band grew and places the next sibling correctly. The remaining
candidates are what the fixture does not model: the `live_block` entity
boundary, async row arrival through the real query pipeline, and the nesting
depth Martin reported.)

## legacy line 395 (COVERAGE 2026-07-31)

the org round trip STRIPS INLINE-MARKUP DELIMITERS from literal block
content. `__default__` comes back as `default`; the same loss hits `_x_`,
`*x*`, `/x/`, `~x~`, `=x=`, `+x+`, standalone or embedded in a sentence
("the __default__ profile is used" → "the default profile is used"). Found
outside a test: task #67, via a ~4% flake in
`undo_cycle_task_state_coverage.rs` whose unordered `pick_target`
occasionally selected `block:__default__` and mislabelled the content loss
as an undo failure. Root cause: `render_headline_block`
(`crates/holon-org-format/src/models.rs`) emits `block.org_title()` VERBATIM
when the block carries no marks, so markup-shaped literal text reaches disk
as live org markup; `parse_org_file` then correctly consumes those
delimiters into `MarkSpan`s and the block's content is permanently shorter.
The round trip is FIXED-POINT STABLE (pass 2 re-emits `__default__`), so
this is one-shot data loss, not an echo loop. GAP = COVERAGE:
`holon_block_roundtrip_testing::valid_title`/`valid_body` are `[a-zA-Z0-9
...]` character classes that exclude EVERY org markup character, so
`round_trip_pbt.rs` structurally could not generate the shape. Now covered
by `crates/holon-org-format/tests/org_roundtrip_characterization.rs` (8
tests, un-ignored on fix). FIXED 2026-07-31 (Martin's ruling: verbatim-quote
on render): `render_lossless` in
`crates/holon-org-format/src/inline_marks.rs` quotes every span the parser
would consume in `=…=`, so `__default__` reaches disk as `=__default__=` and
parses back literally (plus a Verbatim mark — a fixed point after one cycle,
and the token stays greppable on disk). Detection reuses orgize with
`extract_inline_marks`' own config, so there is no second emphasis grammar.
The emit is gated by a TOTAL self-check against `expected_reparse` — the
same parser walk with emphasis kept raw — which states the policy exactly:
LINKS MAY ADOPT, EMPHASIS MUST STAY LITERAL. Applies to marked blocks too
(the literal gaps between marks were equally lossy). COVERAGE gap closed at
the source: `valid_title`/`valid_body` now carry a weighted
`markup_shaped_literal()` arm, verified red-without-the-fix in
`round_trip_pbt` and `org_block_round_trip_pbt`.)

## legacy line 420 (PERCEPTION 2026-07-31)

the FIRST fix for the entry above shipped three defects a fresh-context
verifier caught, all invisible to the tests that certified it. (a) Its
self-check compared the re-parse byte-for-byte against the input, so any
content mixing a literal link with emphasis (`a *b* and [[c]]`) failed the
check and PANICKED render — inside the org-sync select loop, which an unwind
stops vault-wide. (b) The quoting was wired only into the no-marks path on a
provenance argument ("marks only come from the org parser") that is FALSE —
Peritext reads, block-split, template instantiation and the markdown
adapters all mint marks, and the Verbatim mark the fix itself mints puts a
healed block into the unprotected path. (c) An early return skipped the
check entirely whenever there was nothing to quote. GAP = PERCEPTION: every
test asserted on content the FIX chose to quote, none asserted the check ran
on content it chose not to. Now pinned by `render_lossless_shapes.rs` (33
adversarial shapes, no input may skip the check) plus unit rungs for the
marked/healed/externally-minted paths.)

## legacy line 432 (PERCEPTION 2026-07-31)

the SECOND fix for the entry above shipped a composition bug WORSE than the
original: co-extensive marks emitted mis-nested delimiters.
`render_inline_marks` sorted same-position closes by `Reverse(start)`, which
TIES for marks sharing a span, so Bold+Verbatim over one span emitted
`*=x*=` — non-LIFO, not valid org, so the next parse swallowed the
delimiters INTO the content and the block converged to `*=__init__*=`. The
state was one the fix MANUFACTURED (quoting a bolded identifier re-ingests
as Bold+Verbatim co-extensive), and the degraded path re-emitted the very
bytes the checker had just rejected, so content was polluted rather than
preserved. The nesting bug was general and pre-existing — plain `hello` with
Bold+Italic mis-nests identically — and only became reachable because the
fix started minting Verbatim. GAP = PERCEPTION, and structural:
`assert_render_is_fixed_point` was only ever called with `marks=None`, NO
generator anywhere produced arbitrary mark sets (every mark in every test
came from parsing org text, which can only mint states the parser can
express), and no property ran a marked block through TWO cycles — so three
review rounds walked through the same hole. Closed by
`marked_content_strategy` in `holon-block-roundtrip-testing`: `(content,
marks)` store states with marks minted INDEPENDENTLY of parsing, spans
weighted onto the adversarial geometries (co-extensive, crossing,
boundary-aligned, contained-with-slack, inside, duplicate), driven through
>=2 full render->parse cycles against the PROD emit path. Acceptance was
would-have-caught, not just green-after: against the pre-fix build the
generator finds the corruption on case 0 and shrinks it to the minimal pair
— content `"a"`, marks `[Bold{0,1}, Italic{0,1}]`.)

## legacy line 452 (COVERAGE 2026-07-31)

the THIRD fix for the entry above destroyed data on shapes that only a REAL
vault contains, found by simulating ingest->write-back over Martin's live
vault. (a) The expectation let raw link syntax adopt UNCONDITIONALLY,
ignoring a `Verbatim` mark that says "this span is literal". The vault line
`Rule fork F5: raw link form — bare =[[uuid][Label]]= vs page-name sugar`
(18 occurrences) was therefore judged WRONG when emitted correctly,
degraded, lost its `=` quoting, and would next cycle have become a live link
to a nonexistent page. (b) The quoting pass ran inside `Link` mark spans, so
`[[https://example.com][the __init__ method]]` failed its check and the
degraded rung — which dropped ALL marks — DELETED THE URL from disk and
store, unrecoverable. Root cause of both: a mark taxonomy that existed only
in the author's head. Now explicit as `MarkClass` on `InlineMark`
(holon-api): STYLING is droppable, PROTECTIVE changes what a span MEANS,
DATA-BEARING carries payload stored nowhere else — so a new variant cannot
silently inherit the wrong policy. `expected_reparse` takes marks and
suppresses adoption inside non-styling spans; the ladder drops styling, then
protective, and NEVER data-bearing, with every rung verified against the
expectation from the ORIGINAL marks (verifying against the degraded mark set
makes each degradation self-justifying). GAP = COVERAGE: every corpus in the
repo was written by someone who already knew which shapes were interesting.
Closed by `vault_writeback_stability.rs` — ingest->write-back over a real
vault, zero changed lines — plus link syntax and `Link` marks in
`marked_content_strategy`. Would-have-caught: against the pre-fix build the
vault sim reproduces the F5 line verbatim and the generator finds it too.
METHOD NOTE: the round-3 PBT used the implementation's own contract function
as its oracle, so when that function was wrong the property stayed GREEN
through both kills. The generator now carries `expected_after_cycle`,
computed from the segments it assembled — an oracle that asks the code under
test what "correct" means degrades in lockstep with it.)

## legacy line 475 (COVERAGE 2026-07-31)

an INDENTED body lost its FIRST line's indentation on every org round trip
while later lines kept theirs, so ` a\n b` came back `a\n b` — silent,
cumulative re-alignment of any indented note or code-ish body, and the
doc-root preamble had the same defect. Root cause: `str::trim` used as
"strip surrounding blank lines" in three places (`extract_image_links` on
the parse side; `render_headline_block`'s body and
`OrgRenderer::render_document`'s preamble on the render side) — `trim` eats
leading spaces of the first content line, which is indentation, not padding.
Fixed by a shared `models::trim_blank_lines` that removes whole
WHITESPACE-ONLY LINES at both ends and preserves every surviving line's own
indentation; all three sites now share it, so parse and render agree and
`render(parse(render(x))) == render(x)` still holds
(`test_render_string_stability` was the guard that caught a newline-only
first attempt). GAP = COVERAGE: `valid_body` is `[a-zA-Z0-9
.,!?\n]{10,200}`, which CAN emit leading spaces, but the round-trip PBT
normalizes bodies before comparing, so the asymmetry was invisible to it.
Now covered by `indented_first_preamble_line_survives_roundtrip`.)

## legacy line 489 (PERCEPTION 2026-07-30)

secondary ORACLE: the ClaudeCode page rendered its section HEADLINES but
ZERO rows under every query section, and re-navigating never recovered — the
original "blank page" bug (yesterday's `query_block` variant had the same
defect, minus the headline). Live diagnosis: the ViewModel held every row
(`describe_ui` full) while a real-window screenshot showed constant-height
blank bands whose height did not vary with row count. Root cause:
`ReactiveShell`'s block-mode arms unconditionally render
`div().size_full().overflow_y_scroll()`
(`frontends/gpui/src/views/reactive_shell.rs`), a shape valid ONLY for a
PANEL-parented shell living inside `columns::panel_wrap`'s absolute
`size_full`. A query block embedded as an outline ROW via the
`query_block_titled` profile variant
(`assets/default/types/block_profile.yaml`) creates a NESTED shell whose
parent height is indefinite, so `height: 100%` collapsed to a fixed empty
band and no row was ever laid out. PERCEPTION: every model-layer oracle was
satisfied — the rows were in the ViewModel, the query returned them, the
data was correct; only the painted geometry was empty, and nothing in the
suite looked at the height of a nested live_block's band. Secondary ORACLE:
the windowed harness could already read `BoundsRegistry`, so the invariant
was expressible — it just did not exist. Closed by `ReactiveShell`'s typed
`ShellPlacement` (`Panel` keeps today's `size_full` + scroll viewport;
`Nested` is content-sized with no independent scroll) plus a red-first
windowed geometry PBT,
`frontends/gpui/tests/gpui_window_slice.rs::nested_live_block_paints_the_rows_its_model_holds`,
which asserts the MODEL holds the rows and THEN that at least one is painted
at a hit-testable height — so a model regression and a height regression
stay distinguishable.)

## legacy line 509 (ORACLE 2026-07-30)

secondary PERCEPTION: a sidebar tree row's leading chrome sits BELOW its
first text line — measured chevron center-y 18.0px and leaf-bullet center-y
16.0px against a first-line center of 13.0px (`text_line_height` 26). On a
WRAPPED row the error is structural, not merely 3–5px: the bullet's box was
`tree_item_min_height` and the chevron's a `tree_chevron_size` box plus a
hand-tuned `CHEVRON_TOP_OFFSET = 8.0` whose own comment admitted the value
was "pending Martin's live visual pass". ORACLE, not PERCEPTION: the harness
could always express this — the chevron lane added windowed geometry
assertions the same day, but they assert PRESENCE, GLYPH and OPACITY, never
POSITION relative to the text, so a marker could drift anywhere in the row
and stay green. Secondary PERCEPTION only for the taste-calibrated pixel
offset. Closed by `frontends/gpui/tests/tree_row_first_line_alignment.rs`
(red first with those exact numbers, over a two-line row, incl. the
production `selectable(row(icon, spacer, text))` sidebar shape) plus a
one-line-tall marker slot in `tree_item.rs` that deletes the magic offset.)

## legacy line 521 (COVERAGE 2026-07-30)

secondary ORACLE: the creation-slot virtual row leaked into the LEFT SIDEBAR
tree — Martin's live vault rendered 27 rows for 26 pages, the extra one an
empty `block:__virtual:<page>` bullet nested under its parent, which then
fired a `[tree-desync]` ERROR (`in provider but not row_map`) on all 6
disclosure toggles and a "Breadcrumb unavailable: … has no path in
block_with_path" banner. Root cause: `virtual_child_slot_from_arg`
(holon-frontend/src/shadow_builders/prelude.rs) built a slot for ANY tree,
deriving the container from the rendered rows when the spec named no
`virtual_parent`; that made `resolve_creation_parent`'s flat-shape test true
by construction, so the `allow_root_creation` gate the earlier BugFunnel
#61/#67 fixes added was unreachable. COVERAGE: the generator's only tree
render-expr (`reference_state.rs::valid_render_expressions`,
`generators.rs`) always sets `creation_slot: true`, so the read-only sidebar
SHAPE — a tree WITHOUT the flag over a nested-page forest — was never
generated. Secondary ORACLE: `inv-viewmodel-tree-virtual-slots` asserts a
slot's POSITION (last child) but never its ABSENCE on a non-opted-in
collection. Closed by `crates/holon-frontend/tests/sidebar_creation_slot.rs`
— red first with exactly `["block:__virtual:pageA"]` and 4 rows for 3 pages
— plus the opt-in gate in `prelude.rs`.)

## legacy line 598 (ENVIRONMENT 2026-07-31)

`BlockSchemaModule::ensure_schema` DROPped `block_tags` / `block_requires` /
`advice_suppressed` on EVERY boot — persisted projected state destroyed at
each start while `block_raw` (CREATE IF NOT EXISTS) survived. PROVEN by a
new deterministic unit red
`schema_modules.rs::block_junction_schema_is_non_destructive_across_boots`
(0 rows after the second `ensure_schema` where 1 was written; `block_raw`
intact). Only `task_blockers` (the genuine pre-rename legacy name) is still
dropped. Annotate evidence: `block_tags.sql` has NEVER changed shape, so no
migration ever justified its drop; `block_requires.sql` did lose an FK on
`required_id` 2026-07-22, a one-time change the drop-every-boot performed by
accident and every DB booted since has absorbed. NOT the full explanation of
the dogfood symptom — see the ledger row.)

## legacy line 602 (ENVIRONMENT 2026-07-29)

cold boot of a vault whose ONE dominant file holds ~24k blocks took ~47
minutes with ZERO output, blowing every latency budget in silence. Two
independent halves, one escape. Cost: the ingest creates pass calls
`create_in_tree` per block, and its first step —
`LoroBackend::resolve_to_tree_id` — MISSES for every genuinely-new id and
then walks all live tree nodes (`find_tree_id_by_stable_id`,
`tree.get_nodes(false)` + per-node meta read), so one file's creates pass is
O(blocks × nodes); each create also took its own `doc.commit()`. Measured
with the new one-file harness knob (release, Loro, branching 8): 4k blocks =
2.5s, 16k = 37.3s (14.8× for 4× the blocks), per-2k-block slices inside ONE
file climbing 1.0s → 8.0s. Silence: every progress line and the 30s
no-progress watchdog sat at the per-FILE scan loop, so inside one file
'slow' and 'wedged' were indistinguishable — the 47 minutes could not be
attributed until the intra-file lines existed. ENVIRONMENT primary: no test
environment had ever contained a single huge file. Every fixture and the
vault-shaped corpus knob were many-small-files, and the cost regime is a
function of ONE file's node count, so the quadratic term could not appear at
test scale however many cases ran — the missing piece is a scale rung
(one-file × N-blocks), now `HOLON_SOAK_ONE_FILE_BLOCKS` in `diag_harness`.
ORACLE secondary and independent of the rung: nothing bounded ingest work,
so even at vault scale no invariant would have gone red — now three opt-in
budgets (`HOLON_SOAK_BOOT_BUDGET_MS` wall time,
`HOLON_SOAK_MAX_CHILDREN_READS`, `HOLON_SOAK_MAX_CREATE_COMMITS`), the two
count budgets being load-independent observables. Remedy landed: intra-file
progress lines every 2,000 blocks + an intra-file no-progress watchdog that
fires DURING a wedge, and a chunked batch create (`create_in_tree_batch` →
one stable-id-cache warm + one Loro commit per 2,000-block chunk) — 16k
blocks: ingest 37,325ms → 9,690ms, create commits 16,000 → 8. Still open,
fork-side: the turso recursive-CTE cursor O(N²) that makes the doc-scoped
`get_blocks` walk expensive on a huge doc is a Turso-fork item, untouched
here.)

## legacy line 613 (ENVIRONMENT 2026-07-29)

the org-scan sync gate ESCAPES after 600 s and then contends with the very
scan it was protecting — and its own ERROR text predicts the contention it
is about to cause: `sync gate never opened — org initial scan may be wedged;
proceeding with sync in DISCLOSED degraded mode (may contend with the scan)
waited_s=600`. The disclosure is honest (it is a fallback that announces
itself, per the error-handling policy) but the PREMISE is wrong in the
ordinary case: the scan is not wedged, it is legitimately slower than 600 s
on any real vault, so the escape fires on EVERY real-vault cold boot —
observed in both independent boots on hand, the debug acceptance run at
`18:41:39` and the release baseline at `16:17:20`, i.e. the timeout is not a
rare-pathology guard but the default path. In the acceptance run the escape
then loosed 62 `claude-history://projects` resync iterations
(`resync_by_uri: starting`, each a `QueryableCache` batch apply) into the
SAME serialized `TursoBackend` actor the scan is saturating — the actor is a
single-threaded command queue, so every resync interleaves with the scan's
matview reads. This lane's controls show that contention did NOT cause the
observed stall (the release baseline had `resync_by_uri` count 0 and was
equally slow), so this row is the LATENT hazard, not the stall's cause — but
it is a real one: the escape is scheduled precisely when the scan is at its
most expensive, and it grows with vault size while the 600 s constant does
not. ENVIRONMENT primary: the gate/escape path is boot-ordering wiring that
the keystone never exercises — its fixtures complete the scan in
milliseconds, so `waited_s` never reaches 60, let alone 600, and no
transition sequence can make it. NOT an oracle gap: had the escape fired in
a test, `inv-no-observed-errors` would have caught the ERROR immediately —
it is unreachable, not unjudged. NOT FIXED — the policy fork is Martin's to
rule: (i) hold the gate until the scan completes with no wall-clock escape
(correct-by-construction, but a genuinely wedged scan then blocks
integrations forever, trading a visible degraded mode for an invisible hang
— would need the scan's own liveness signal to justify), (ii) keep the
escape but back off resyncs while `in_initial_scan` is true (preserves the
"integrations eventually work" property and caps the added load, but leaves
two writers on the actor and needs a backoff constant that is itself
scale-dependent), or (iii) make the gate progress-grounded rather than
wall-clock — escape only when the scan makes NO progress for N seconds,
which is what `finish_initial_scan`'s existing no-progress watchdog already
does for the feed and would have kept the gate closed in both observed
boots, since both were progressing the whole time. (iii) is the
recommendation: it makes the ERROR's "may be wedged" claim actually true
when it fires.)

## legacy line 614 (ENVIRONMENT 2026-07-29)

the 1,472 s cold-boot readiness gap (ENV row below) is NOT paid by the org
scan's own per-file batching — it is paid by the Loro->SQL projector run
loop reconciling CONCURRENTLY with the scan. Measured from the PINNED
dogfood log (`/private/tmp/holon-cold-PINNED-2026-07-28T1940.log`, boot
window 16:07:19.94-16:31:52.68): 16,333 `LoroProjection::project` passes, of
which only 37 sit inside an `org.ingest_file` span. The scan's own cadence
is ONE flush per file (`file_sync_controller.rs` ingest-path
`downstream.flush()`, ~1,001 passes for the vault); the other 16,296 passes
are the run loop waking once per Loro commit, each paying a full
sibling-scope snapshot (`snapshot_ms` 565 s of the 906 s apply total) and
its own SQL transaction. The run loop is supposed to start only AFTER the
scan: `holon-app/src/wiring.rs` resolves `LoroSyncControllerHandle` inside
`post_ready`, behind the `FileWatcherReadySignal`, and
`frontends/gpui/src/main.rs:82` documents exactly that ("resolved by the
FrontendSession factory in a background task that awaits OrgMode readiness
first"). ~100 lines later the same file's MCP debug-handles cell does
`runtime.block_on(injector.try_resolve_async::<LoroSyncControllerHandle>())`
unconditionally — which RESOLVES the provider, and the provider's factory
calls `controller.start()`. Log proof: `Session ready` 16:07:22.851573,
`[LoroModule] STAGE 2: LoroSyncControllerHandle factory body started`
16:07:22.851595 (22 microseconds later), `[LoroSyncController] Started
outbound Loro->SQL reconcile loop` 16:07:22.870157 — i.e. 24 minutes before
`[post_ready] org scan complete`. ENVIRONMENT primary and unusually literal:
the headless fixture CANNOT reach this state. `TestEnvironmentBuilder` grabs
the handle with the SYNCHRONOUS `try_resolve`, which returns `Err` for an
`async`-provided service nobody has awaited yet; with the default
`wait_for_file_watcher(true)` that `Err` is invisible because `post_ready`
already awaited the handle before the builder looks. Flip the fixture to
prod's `wait_for_ready=false` and the sync `try_resolve` silently yields
`None` — the fixture boots with the projector DEAD, the exact opposite of
prod, and no assertion notices. Reproduced in `diag_harness` (new
`HOLON_SOAK_VAULT_FILES` / `HOLON_SOAK_VAULT_BIG` vault-shaped corpus +
`HOLON_SOAK_PROD_BOOT=1`, which awaits the handle the way GPUI does) and
measured with new log-level-independent counters
(`holon_loro::loro_sync_controller::projection_stats`; the per-pass
`holon_latency` events are `debug!` and `release_max_level_info` compiles
them out of every release build, so boot cadence was unmeasurable at scale).
A/B on one 14,012-block vault-shaped corpus (300 files, dominant file 6,000
blocks): run loop dead = 301 passes / 48.6 ops per pass / 131.5 s; run loop
live = 2,317 passes / 6.17 ops per pass / 186.5 s — 7.7x the passes and +42
% wall for byte-identical input. FIXED: the gate moved ONTO the controller
rather than onto each caller's discipline —
`LoroSyncController::start_gated` spawns its run loop behind
`holon_core::SyncGate` (`crates/holon-loro/src/loro_sync_controller.rs`),
wired at the single DI seam `crates/holon/src/sync/loro_module.rs`.
Resolving the handle still returns immediately, so GPUI's `runtime.block_on`
cannot deadlock; `post_ready` opens the gate on every scan-completion path;
an all-holders-dropped gate returns `SyncGateClosed` and the loop starts
disclosed-degraded rather than stranding the projection forever. The three
eager resolve sites are left AS IS and are now harmless — putting the gate
at the callers would have left a fourth caller free to reintroduce it. The
Loro subscription stays UNgated on purpose: gating it too would leave
`pending` empty during the scan and route every per-file flush to a full
reseed (visible in the pre-fix control column below as 26-35 s of snapshot
time for 301 passes). Red-first regression
`crates/holon-integration-tests/tests/boot_projector_gated_on_scan.rs` — RED
at `gate-red-projector-gate.log` ("projector run loop started while the org
initial scan was still in progress (SyncGate deferred)", with a non-vacuity
guard that fails if the scan outran the probe), GREEN at
`gate-green-projector-gate.log`; it also asserts the loop DOES start after
ready, so the inverse bug cannot pass. Re-measured on the same 14,012-block
corpus, same session: prod-faithful boot went 2,317 passes / 186.5 s -> 281
passes / 65.1 s, snapshot time 22,541 ms -> 145 ms, apply 124,309 ms ->
19,301 ms; it now also beats the run-loop-dead control (301 passes / 100.0 s
/ 26,082 ms snapshot) because the ungated subscription lets the scan's
flushes take the incremental path. Residual gap disclosed: the fixture
reaches 47 % single-op passes vs prod's 87.2 % — prod's per-block ingest
work is ~10x the synthetic corpus's, so prod's writer never outruns a
projection pass and the ping-pong is total; scaling the dominant file alone
does not close it)

## legacy line 616 (ORACLE 2026-07-28)

the four `DECLARED column absent from row` WARNs still fired on EVERY cold
start after the backlinks widening three rows below — same four signatures
(`bullet_shape`/`collapsed`,
`is_rule_head`/`is_holon_source`/`is_legacy_rule`/`source_language`),
byte-identical. REFINES that row's diagnosis: the backlinks projection was
genuinely narrow and its fix stands, but it was never the producer of these
four. Producer identified by instrumenting the warn site with the row's live
scope: the row is `block:__virtual:journals` — the SYNTHETIC creation-slot
row the frontend builds IN PROCESS
(`reactive_view.rs::creation_slot_keyed_row`, mirrored by
`shadow_builders/prelude.rs::virtual_child_row`), carrying only
`id`/`parent_id`/`sort_key` plus whatever `virtual_child: defaults:` the
profile YAML sets — for `block` exactly `content` and `content_type`
(`assets/default/types/block_profile.yaml:151`). So there is no query to
widen: the slot row was a NARROWER PROJECTION OF THE BLOCK ENTITY THAN ANY
REAL ROW, and every computed field over a declared column the YAML happens
not to list was unbound on it. That also explains the timing that refuted
the vault-data hypothesis — the slot renders at first paint, ~1.5 s in, long
before org ingest. Consequence is the same silent-wrong render as the
backlinks case, confined to the trailing "type here to create" row. ORACLE
and unambiguously so: the keystone GENERATES this row
(`generators.rs:516,531` emit `creation_slot: true` collections) and asserts
its POSITION (`viewmodel_tree_virtual_slots`), but no invariant asserts its
COLUMN SHAPE. Fixed by making the slot row's declared shape derive from the
schema instead of the YAML: `VirtualChildConfig::widened_to_declared` seeds
every declared column the YAML omits with `Null` — the value a projected row
carries for an unset column — normalized through
`EntityProfile::with_widened_virtual_child` at `ProfileCache::new`, the ONE
funnel every profile source (type-defined, org-sourced, merged) passes
through, so an un-widened slot config cannot reach a resolver; and both row
builders now overlay defaults BEFORE the structural
`id`/`parent_id`/`sort_key` so identity always wins. Red-first by
`holon-profiles::creation_slot_defaults_cover_every_declared_column` (red
naming `collapsed`, `created_at`, `id`, `marks`, `parent_id`, `properties`,
`source_language`, `source_name`, `updated_at` — `id`/`parent_id` appear
because they too are declared columns, which is exactly why the builders now
overlay defaults BEFORE writing structural identity). Acceptance: fresh
isolated cold boot, 0 occurrences in the first 2.5 min (was 4/4). Remaining
gap UNCHANGED and now twice-earned: still no keystone invariant asserting
zero `DECLARED column absent` lines — two different producers have now
escaped through the same missing oracle.)

## legacy line 620 (ORACLE 2026-07-28)

the "Linked references" accordion renders backlink rows through the `block`
entity profile against a THREE-COLUMN projection, so four of that profile's
computed fields are permanently unbound on every cold start —
`holon_api::computed` warns `DECLARED column absent from row` for
`bullet_shape`/`collapsed` and for
`is_rule_head`/`is_holon_source`/`is_legacy_rule` on `source_language`.
Consequence is silent-wrong render, not a crash: every backlink row falls
back to the plain `circle` bullet and is never classified as rule/program
machinery. Two projections were narrow: the `backlinks` matview itself
(`target_id, id, parent_id, content, content_type`) and the seeded
live_query on top of it. ORACLE, not COVERAGE or ENVIRONMENT: the harness
seeds the SAME accordion (`seed_wide/index.org`) and the keystone reaches
focused pages with incoming links routinely, but NO invariant asserts that a
rendered row carries the columns its entity profile declares — the only
signal was a `warn!` nobody asserts on. Fixed by deriving the backlinks
matview projection from `BLOCK_RAW_COLUMNS` and widening the seeded query to
`SELECT bl.*`; pinned red-first by
`schema_modules::tests::backlinks_view_projects_every_block_column` and
`backlinks_section_seed::backlinks_section_query_projects_whole_block_row`.
Remaining gap: still no keystone invariant over the warn itself.)

## legacy line 621 (ENVIRONMENT 2026-07-28)

the `claude-history` MCP integration registers INERT on every cold start
(`WARN Integration 'claude-history' unavailable`) because the config the app
actually loads — `~/.config/holon/integrations/claude-history.yaml` — is a
stale hand-made copy in which `session` and `task` declare BOTH a `sync`
strategy and `vtable.write_through`, which `finish_integration`'s clash
check correctly rejects. The IN-REPO `docs/integrations/claude-history.yaml`
is already compliant AND already pinned by
`mcp_fanout::claude_history_yaml_multi_project_shape`, so no test could ever
have caught this: ENVIRONMENT — there is no install/sync path from the
repo's canonical integration yamls to the config dir the runtime reads, and
the two have silently diverged since 2026-04-21. Remedy is an install step
(or a boot-time disclosure that the loaded yaml is older than the shipped
one); repo side needs no change.)

## legacy line 623 (ENVIRONMENT 2026-07-28)

a SINGLE `navigation.focus` write WEDGES the Turso IVM actor for 23+ MINUTES
at real-vault scale — commit cost scales with total matview state, not with
delta size. Found by memory-growth profiling of the GPUI app against a copy
of Martin's real vault (102 files / 24,369 blocks, of which ONE file —
`Projects/Holon.org` — carries 24,319). Two `sample` captures 90 s apart
show the IDENTICAL stack, so this is a wedge or an effectively-unbounded
computation, not slow progress: `TursoBackend::process_actor_command` →
`handle_query` → `Statement::step` → `vdbe::Program::normal_step` →
`op_halt` → `halt` → `commit_txn` → `apply_view_deltas` →
`IncrementalView::merge_delta` → `DbspCircuit::commit` → `run_circuit` →
`execute_node` ×12 NESTED (the chained-matview DAG walked in full per
commit) → `JoinOperator::commit` (3577/4063 then 3158/3438 samples) →
`Delta::consolidate` (2822 then 2481 samples) →
`HashMap<HashableRow,i64>::entry` + `reserve_rehash`. KEY LINE:
`Delta::consolidate`'s working set is not bounded by the delta — a one-row
navigation write re-consolidates a join delta sized by the whole store,
which is a full recompute inside what is contractually an INCREMENTAL
circuit. Corroborated by the memory trace: a ~1.14 GB allocation burst in
~10 s (892→975→2035 MB) during ingest, released once commits stop —
transient DBSP delta state, exactly what an unbounded consolidate would
allocate. Same cause drives the throughput numbers, ALL orders of magnitude
past the p95 interaction→projection-visible < 200 ms SLO: ~9 s per
navigation focus, ~34 s per block edit, ~5 s per query, 5.9 s for `SELECT id
FROM block_raw WHERE parent_id=?` over 24k rows, and +141 MB/min sustained
during a 698 s boot ingest (`boot_parse` alone logged 13,995 ms for one
file). ENVIRONMENT primary per the rubric's explicit latency rule (budget
holds at test scale, not at vault scale) AND its named "real-vault scale"
clause: the interaction is fully generatable — the keystone draws focus
transitions constantly — but the failing regime is entered only when total
matview state is large, and the keystone's scale knob
`HOLON_SOAK_SEED_BLOCKS` (`composed/soak_seed.rs:217`) DEFAULTS TO 0, so no
default run has ever been within three orders of magnitude of it. Explicitly
NOT a coverage gap: nothing about the transition sequence is missing, only
the store size. ORACLE secondary, and it is the durable half: there is NO
commit-duration or projection-latency budget invariant anywhere in the
catalog, so even a soak-seeded run would sit there for 23 minutes and
eventually report GREEN rather than red — a test that cannot distinguish
"correct" from "correct after 23 minutes" is not an oracle for a latency
SLO. Remedy in two parts: (1) ORACLE — add a per-transition commit/settle
budget invariant that FAILS (not warns) past the 200 ms SLO, which makes the
gap detectable at any scale; (2) ENVIRONMENT — run the keystone with
`HOLON_SOAK_SEED_BLOCKS` at vault scale (~25k) in a soak lane so the budget
invariant actually enters the regime. Prod-side fix is NOT ours: routed to
the Turso fork owner via the handoff addendum
`/private/tmp/turso-ivm-joincommit-fullrecompute-2026-07-28.md`, which
carries both stack samples verbatim and the falsifiable claim; a SQL-level
reproducer looks tractable since the wedge is reached by a plain single-row
write against a chained-matview schema with a large base table. POSSIBLY
RELATED but explicitly NOT claimed identical: the open anti-join
retract-race handoff `/private/tmp/turso-ivm-race-handoff-2026-07-27.md` —
same operator family (join/anti-join `commit`), different symptom (that one
is a correctness race, this one is unbounded cost). Evidence:
`/Users/martin/.claude/jobs/72446a9c/tmp/hang-sample.txt` +
`hang-sample2.txt` (the two stacks), `memprofile-rss.tsv` +
`memprofile-fp.tsv` (5 s RSS/phys_footprint trace, phase-tagged),
`memprofile-app.log`, `phases-run1.log` + `phases-run2.log`. Profiled
against workspace `soak-main-0728` @ `7c9aabb07dfd`, main `68f4d452`. NOTE
FOR REVIEWERS: this wedge makes the app unusable at Martin's real vault
size, and it truncated the memory-growth profile it was found by — the
entity-view-registry / ListState / LiveQuery-cache / Loro-oplog hypotheses
were all measured NEGATIVE (−4.7, +2.3, −0.8, −2.6 MB/min against +3.8
MB/min idle noise) but only at 34 navigations / 260 scrolls / 65 queries /
10 edits, because throughput starvation made larger op counts impossible;
those hypotheses are PARKED, not cleared, and must be re-tested after this
fix.)

## legacy line 624 (ENVIRONMENT 2026-07-28)

`click_entity` fails "element bounds never committed; stale focus cleared to
prevent silent mis-targeted typing" against the LIVE GPUI app, killing any
live-MCP keystone walk that draws a click-dependent gesture — and, as a
CONSEQUENCE, leaving the reorder transitions silently dead. Reproduced on
every default-weight live run (dies at case 1, ~20 s, on `SplitBlock`'s
focus click; seen on both `block:fe-target` and a minted
`block:8a62e2ae-…`). Root cause is a prod/test parity gap, not a flake: the
headless keystone clicks through `ReactiveEngineDriver` + click-intent
resolution, where every block is reachable and no LAYOUT exists; the
windowed app commits bounds only for blocks the GPUI render loop has laid
out, which after the Inc 5 main-outline virtualization (`gpui::list`,
O(viewport) rows) is the VIEWPORT ONLY — so the driver picking an arbitrary
oracle block clicks something with no bounds. ENVIRONMENT primary per the
rubric's tiebreaker: the interaction IS generatable (it generates and fails
within 20 s), but the failing code path — bounds commitment — does not exist
in the headless wiring at all. COVERAGE secondary, and it is the
load-bearing half: with the click-dependent gestures ZEROED to dodge this
bug, `MoveUp`/`MoveDown` become precondition-DEAD in the live rung (both
require Main focus ON the block itself, non-page, with a previous sibling,
and only `FocusEditableText`/`ClickBlock` — both click paths — can put focus
there), so the reorder caps are registered-but-unreachable exactly like the
`InstantiateTemplate` case: SECOND instance of that class. Proven, not
inferred: a mutation probe replacing the looked-up action with
`move_up_MUTATION_PROBE` (a name the registry cannot hold) left the live run
GREEN, and direct instrumentation of `send_reorder_chord` +
`apply_focus_editable_text` recorded 0 hits each at
`MoveUp:100,MoveDown:100,FocusEditableText:60` over 4 cases. NOTE FOR
REVIEWERS: live-rung reorder coverage therefore rests ENTIRELY on the
`mcp_list_keybindings_matches_registry` wire-contract guard plus the
`holon_api::Key` round-trip units — the keystone contributes nothing, and a
green live run must NOT be read as reorder coverage. Remedy
(ENVIRONMENT-parity, per CLAUDE.md's make-prod-and-test-more-similar rule):
scroll-into-view before clicking in the live driver, or restrict the live
generator to blocks with committed bounds and let the oracle model
visibility — either way `MoveUp`/`MoveDown` regain a reachable path and the
mutation probe must go RED. Found while verifying the `list_keybindings`
collapse.)

## legacy line 625 (ENVIRONMENT 2026-07-28)

a PANIC inside an rmcp tool handler NEVER sends a response — the MCP client
blocks FOREVER instead of seeing an error. Observed twice in one session
while building `render_org`: both a `debug_assert` in `EntityUri::block` and
a `ProjectionInvariantViolated` panic in `render_walk` killed the handler
task, and the caller (`McpUserDriver::call_tool_json`, and equally a
dogfooding agent) simply never returned — the test binary sat parked on a
tokio `block_on` with every worker idle. The panic IS printed to the app's
stderr, so the fault is loud in the log and invisible on the wire; diagnosis
required `sample`-ing the hung pid to find the blocked frame, then
re-running under `RUST_LOG` to catch the stderr line. Cost the large
majority of this increment's debug time, and it silently converts EVERY
future handler fault into a wedged agent/suite. ENVIRONMENT primary: this
failure mode belongs exclusively to the OUT-OF-PROCESS rung — in-process
caps propagate a panic and fail the test immediately, while the same panic
across the MCP wire becomes an unbounded wait, so no headless test can
observe it and the live rung hangs rather than reds. COVERAGE secondary: no
fault-injection transition deliberately faults a handler, so even the live
rung never enters the path. OPEN, wants its own ticket. Remedy:
`catch_unwind` at the rmcp handler boundary mapping a panic to an
`internal_error` response carrying the payload, PLUS a per-call timeout in
`McpUserDriver` so a wedged tool fails loud instead of hanging the suite
(defense in depth — the client-side timeout also covers a genuinely stuck,
non-panicking handler). Found while building the faithful `render_org`
surface. SIGHTING 2026-07-28 (independent, predicted by this row): during
`list_keybindings` live verification a `tracing-subscriber` internal panic —
"tried to clone a span (Id(…)) that already closed",
`registry/sharded.rs:317`, on a tokio-rt-worker after the 4th per-case
`reset_vault` — killed a handler task and WEDGED a 4-case live-MCP run for
~10 minutes with ZERO wire-side signal; 3 of 4 cases had already gone green,
so the run looked merely slow. Diagnosis again required `sample`-ing the
hung pid and then re-running under `RUST_LOG` to find the stderr line. Two
escalations this row did not originally capture: the panic source is a
DEPENDENCY (not Holon code), so no amount of Holon-side care prevents
recurrence — only the handler-boundary catch + client timeout do; and heavy
per-case span churn across `reset_vault` is a reproducer, so the live
keystone is the most likely place to hit it again.)

## legacy line 626 (COVERAGE 2026-07-28)

`HolonMcpServer::resolve_doc_uri` (`tools.rs:3860`) calls
`EntityUri::block(doc_id)` on an ALREADY-SCHEMED id — `debug_assert`s "got
an already-schemed id" in debug builds, and in RELEASE mints the
double-schemed `block:block:<uuid>`, which matches no row. Reachable today
by the documented agent workflow: `list_loro_documents` publishes aliases in
SCHEMED form (observed: `block:ref-doc-0`), and the helper's sole surviving
caller is `diff_loro_sql` (`tools.rs:2245`), so feeding a listed alias
straight into the Loro↔SQL diff tool either panics the handler (debug — and
per the row above that HANGS the client) or silently diffs a nonexistent id
(release). Sibling of the same-day dense_patch arg-name row: a
tool→id-construction seam with zero test traversal. COVERAGE primary: no
transition invokes `diff_loro_sql`, so nothing constructs a doc uri through
this helper. ORACLE secondary, and it is the interesting half — the ONLY
guard is a `debug_assert`, which compiles OUT of the configuration that
ships, so even a covering test would go green in release while prod
double-schemes; a guard that vanishes in the shipped build is not an oracle.
FIXED 2026-08-05 (see the diff_loro_sql row of that date; boundary now
parses idempotently) — was OPEN (flagged, unfixed — `render_org`
deliberately bypasses the helper via the idempotent `EntityUri::from_raw`).
Remedy: make `resolve_doc_uri` use `from_raw` (or return `Result` and reject
loudly), then drive `diff_loro_sql` from the live-MCP rung with a listed
alias. Found while building the faithful `render_org` surface.)

## legacy line 627 (COVERAGE 2026-07-28)

MCP `render_org_from_blocks` was BROKEN ON EVERY REAL VAULT — it fed the
ENTIRE global Loro tree to `OrgRenderer::render_entitys`, whose projection
guard panics `org render: block <id> has dangling parent sentinel:no_parent`
the moment the vault holds more than ONE page root. Holon keeps all pages in
one global Loro doc (`resolve_by_doc_id` ignores its argument and returns
the global doc), so the tool only ever "worked" on a single-page vault; a
real vault (or the keystone seed, which carries structural-page + Journals +
index) panicked every call — and per the handler-panic row above, panicking
meant the caller HUNG rather than erred. Latent since the tool was written.
COVERAGE primary, no secondary: the render guard is a correct, firing oracle
— nothing ever CALLED the tool. The live-MCP `SutOrgRender` pointedly did
not (it re-implemented the render test-side because the tool was
Loro-sourced and headerless), which is precisely how a tool can rot while
looking covered. FIXED in the same increment that removed the duplication:
`render_org_from_blocks` and `render_document` are replaced by ONE
`render_org {doc_id, source: sql|loro, scope: document|blocks}` whose Loro
arm scopes the global tree to the document's subtree via a Page-boundary
walk mirroring `BlockReader::get_blocks`. Now permanently exercised: the
collapsed `SutOrgRender` drives the tool on every org-render fixed-point
check (109 green engagements over 4 live-MCP cases), and
`mcp_render_org_matches_writeback.rs` asserts all four source×scope points
incl. byte-equality against what write-back actually wrote. Found while
building the faithful `render_org` surface.)

## legacy line 628 (COVERAGE 2026-07-27)

MCP `dense_patch` cannot CREATE a block positioned after an existing
sibling, AND the failure is NON-ATOMIC — it leaks an orphan. The create
branch (`tools.rs:2611`) runs `execute_operation("create")` (which COMMITS)
then a SEPARATE `move_block_after(new_id, parent=None, after=sibling)`
(`tools.rs:2644`→`move_block_after` at `:231` omits `parent_id`); that
`move_block` fails with a SWALLOWED generic `move_block on <id> failed:
Failed to execute operation 'move_block'` and the tool returns an ERROR —
but the created block is already persisted, so each failed create+position
patch leaves an orphaned block behind (verified: 3 orphaned `DOGFOOD-TEST`
containers found in the live vault after 3 error-returning attempts; deleted
via `execute_operation delete`). Violates the tool's own "apply as one
batch" contract. dry_run does NOT predict the move: the same case reports
`move_count:0, 3×create` yet execution issues a positioning `move_block`.
Reproduced 3× (top-level AND child-of-page, ruling out sentinel-parent).
COVERAGE primary: NO keystone transition (headless `WideE2E` or live-MCP
`LiveMcpE2E`) invokes dense_query/dense_patch — the whole tool pair is
outside the transition alphabet; the only dense tests cover the PURE
planning logic, not the live tool→op path. ORACLE secondary: swallowed
move_block error + non-atomic partial write + dry_run≡apply divergence.
Remedy: a `DenseProjectionEdit` live-MCP rung (query→mutate dense
text→patch→assert round-trip + no-orphan-on-failure) red-first, then fix the
create-position call (pass resolved parent; wrap create+position in one
transaction) + align dry_run. Found by live-MCP dogfood.)

## legacy line 629 (ENVIRONMENT 2026-07-27)

daily_journal holon_rule does NOT auto-create today's page in the LIVE app —
dogfood-explorer found the Journals page's `Journal Auto-Create` rule
rendered `Enabled` with `last fired: -`, its condition `not
block_exists("Journals/{today}")` SATISFIABLE (no `2026-07-27` block exists,
`journal_feed` holds a single non-dated row, zero `2026-07*` pages ever
existed in this vault), yet after the app had run for hours (discovery_tick
every 2s) and Journals was navigated to repeatedly, no dated journal page
was created and the rule's block properties are empty `{}` (no fire stamp).
This is the CONVERSE of the line-18 escape where the SAME rule fires
AUTONOMOUSLY on PR #99's random walk and hits the page-id collision: the
keystone's wiring FIRES the rule, the live app's rule-scheduler/discovery
path does NOT — a fire-in-test / silent-no-fire-in-prod divergence.
ENVIRONMENT primary (the real holon_rule boot/tick firing path + Rhai action
dispatch runs in the app but not the headless keystone's scheduler, so no
rung observes the live app failing to fire) / ORACLE secondary (no invariant
asserts an enabled rule with a satisfiable condition eventually fires +
persists a last-fired stamp). CANDIDATE confidence — Martin to confirm the
intended trigger (boot vs timer vs on-view) and whether this is a fresh
vault; observed read-only via the live MCP, no repro forced. Remedy: an
ENVIRONMENT-parity rung that boots the real rule scheduler over a seeded
Journals+rule and asserts today's page materializes once. RULED 2026-07-28
(Martin): the eager auto-create is NOT the desired behavior — target design
is a VIRTUAL journal page (proposal page) that renders on view and persists
only when the user first enters data, mirroring the existing `Type here`
placeholder-block pattern; the rule's silent no-fire remains an unexplained
divergence worth understanding, but the fix lane implements the virtual-page
design (feature, red-first) rather than making the eager rule fire;
keystone's eager-fire wiring gets aligned to the new semantics then.)

## legacy line 630 (COVERAGE 2026-07-27)

`InstantiateTemplate` is a DEAD transition — registered in the keystone
alphabet but NEVER drawn in ANY harness, so template instantiation had zero
effective E2E coverage while appearing covered. Evidence trio (verified):
`SutTemplateInstantiate` is registered ONLY in composed/builder.rs:553's
`else if has_turso` (storage-only, no-ViewModel) branch; `set_for_wiring`
(wide_e2e.rs:1260) ALWAYS adds `Projection::ViewModel` when Turso is
present, making that branch unreachable in the composed keystone; grep
confirms no other registration and no slice/test exercises the transition —
cap-narrowing silently deselects it everywhere. Discovered by the
composite-undo lane when its hand-authored red case panicked
`SutTemplateInstantiate absent from the CapMap`. Corollary env gaps blocking
a naive wiring fix: the frontend Loro engine rejects the driver's parentless
template-root seed (mutation_driver.rs:329), and template-definition blocks
`block:tpl*` have no ref-oracle counterpart/exclusion. COVERAGE primary
(alphabet hole behind an unreachable wiring branch); ORACLE secondary
(nothing asserts every registered transition is drawable in ≥1 composition —
silent vacuity; candidate hardening = a non-vacuity draw assert per
registered transition). Remedy tracked: wire a valid-parent template seed +
oracle modeling of block:tpl*, then re-add the undo jsonl case (preserved in
the lane notes); interim red coverage = the engine-level one-undo-gesture
test landed 2026-07-27.)

## legacy line 631 (COVERAGE 2026-07-27)

MCP `dense_patch`/`move_block_after` sent the positional anchor under the
key `position_after_block_id`, but the op layer's macro-generated param
bridge maps params by EXACT arg name (`after_block_id`) and silently DROPS
unknown keys — so every dense_patch positional MOVE (and, once the
missing-parent defect was fixed, every positioned CREATE) reported success
while the row landed FIRST-child instead of after its anchor. Sibling of the
same-day create-after-sibling row (same function, second defect). All other
prod callers use `after_block_id`, so only the dense tool path was wrong.
COVERAGE primary: the tool→op arg-NAME seam had zero test traversal — no
test drove dense_patch through execute_operation, and the param bridge
tolerates unknown keys silently, so nothing could fire. ORACLE secondary:
success-with-wrong-position + the bridge's silent-unknown-key tolerance (a
fail-loud unknown-param rejection at the op boundary would have caught it
instantly — candidate hardening). Red-first: stage-2 of dense-inc1-red.log
(three sibling-order invariants, first-divergent-layer store/CRDT) via the
new `DenseProjectionEdit` live-MCP transition; FIXED same increment (key
rename + Null-anchor convention). Found by live-MCP de-risk probe against
the op registry.)

## legacy line 642 (ENVIRONMENT 2026-07-22)

main-panel scrolling dead — eager content-height column had no
`overflow_y_scroll` viewport in the `columns` flow-panel wrapper,
wheel/trackpad no-op; FIXED, red-first windowed rung `main_panel_scroll.rs`;
COVERAGE secondary — no native-overflow scroll rung existed)

## legacy line 643 (PERCEPTION 2026-07-22)

long block content doesn't reflow — `tree_item` `flex_1` content wrapper
missing `min_w(0)` so `w_full` text never wraps; FIXED, live-verified;
ENVIRONMENT secondary — headless gpui text platform doesn't soft-wrap, so no
windowed wrap rung is expressible)

## legacy line 644 (COVERAGE 2026-07-22)

ID-less external-edit re-ingest duplicates blocks — the file-watch reconcile
keys UPDATE-vs-CREATE by block-id ONLY, so an ID-less headline that gets a
fresh UUID on every parse can never match its already-minted twin; top row;
COVERAGE primary / ENVIRONMENT secondary)

## legacy line 645 (COVERAGE 2026-07-22)

to_create_table_sql keyword-column quoting escape — see row below)

## legacy line 646 (COVERAGE 2026-07-22)

QueryableCache change-origin SQL path keyword-column quoting escape —
sibling of the to_create_table_sql row, was flagged KNOWN-ADJACENT there;
bottom row)

## legacy line 648 (ENVIRONMENT 2026-07-22)

sidebar journal-nesting real-vault verify — journal dates nest correctly
end-to-end, reported "top level" subsumed by the FIXED tree-indent-inversion
(PERCEPTION secondary) + the still-open F5 duplicate-folder-page class,
which was re-confirmed on a fresh real-vault ingest as Areas×2/Music×2,
COVERAGE secondary)

## legacy line 649 (COVERAGE 2026-07-22)

`[[` autocomplete page+title-block duplicate, ORACLE secondary)

## legacy line 650 (PERCEPTION 2026-07-21)

the ingest-heal row landed via the integration chain, uncounted by either
branch)

## legacy line 657 (COVERAGE 2026-08-10)

promotion dogfood send-back (tasks #78/#79): the `cycle_task_state`
vocabulary asymmetry (F3) and the empty-remainder promotion-undo arm (F5).
Counted as two, not one: F5's escape is the windowed rung's SHAPE (it
exercises only a non-empty remainder), F3's is the keystone never cycling
inside a `#+TODO:` document — different generators, different remedies.)

## legacy line 658 (ORACLE 2026-08-10)

promotion dogfood send-back (task #78): no round-trip invariant on the
UNDONE state (F2). Deliberately NOT counted twice with F5 below — F5 and F2
are the same representability class at two layers, and the class is counted
once, under the layer whose remedy is an invariant.)

## legacy line 660 (COVERAGE 2026-08-10)

the `state_toggle` click-path ring row above — counted here so the
leading-token rule stays honest.)

## legacy line 661 (ENVIRONMENT 2026-08-12)

D2 lane: the journal day-page's sidebar click binds no navigate intent past
~2000 blocks (dead affordance + ~5.3s stall), reproduced against a 210-block
control. Counted ONCE, under ENVIRONMENT, although the same measurement also
re-evidences the already-open ordinary-navigate SLO breach (919-1165ms `e2e`
at vault scale) — that is a fresh data point on an existing row, not a new
escape, and is deliberately not counted again.)

## legacy line 662 (COVERAGE 2026-08-12)

det-sched Increment 2 lane: the first armed interleaving run found
`promote-todo-keyword-loro` losing its typed content under concurrent
keystroke dispatch — the unarmed keystone awaits every write and settles
between transitions, so it could never generate two writes of one gesture in
flight (`inv-blocks-match-ref/loro`).)

