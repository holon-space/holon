# Bug Funnel — escape ledger

Every bug found OUTSIDE an automated test gets one row here, classified by the
`bug-gap-triage` skill (`.claude/skills/bug-gap-triage/SKILL.md`). The gap
distribution steers QA investment.

**Running distribution: ENVIRONMENT 14 · COVERAGE 7 · PERCEPTION 3 · ORACLE 1** (as of 2026-07-09)

Gap definitions: **COVERAGE** = keystone couldn't generate the interaction ·
**ORACLE** = generatable but no invariant flags it · **ENVIRONMENT** = prod
wiring/timing/platform differs from test · **PERCEPTION** = visual/UX, no
formal invariant in current harness. Latency-over-budget (SLO: p95
interaction→projection-visible < 200ms) is ORACLE or ENVIRONMENT, never
PERCEPTION.

## Ledger

Seeded 2026-07-07 from the retroactive audit of documented dogfood/triage bugs.

| Date | Bug (one line) | Primary gap | Secondary | Missing piece | Remedy status |
|---|---|---|---|---|---|
| 2026-07-06 | iOS drawer doesn't collapse on nav | COVERAGE | — | no windowed drawer render/driver in keystone | fixed (uncommitted); gap open |
| 2026-07-06 | iOS theme not applied live | COVERAGE | — | no theme-change transition; headless doesn't render Theme global | fixed; gap open |
| 2026-07-07 | iOS add-block: creation slot parents to panel id, engine rejects create | COVERAGE | ENVIRONMENT | no text-sync-on-virtual transition; creation-slot code unreachable | FIXED (landed main e831f0bd): `resolve_creation_parent` resolves the slot parent to the query focus root + keystone `create_block_under_focus` transition |
| 2026-07-05 | GPUI stale sidebar on page-delete | COVERAGE | ORACLE | keystone never deletes a page (`apply_mutation` filters `!is_page()`); sidebar watch never a RefWatch | open |
| 2026-07-05 | page_title-not-h1 / virtual-slot ordering | COVERAGE | — | streaming render path not driven headless | open |
| 2026-07-05 | ToggleState never fires even at 200× weight | COVERAGE | — | generator precondition never satisfiable | open |
| 2026-07-05 | cycle_task_state writes keyword but not task_state_category | COVERAGE | — | invariant exists but gated behind ToggleState coverage hole | open |
| 2026-07-06 | iOS matview error banners at boot (tableless ::trigger:: watch) | ENVIRONMENT | — | boot watch/action_watcher registration not in headless boot wiring | fixed (uncommitted) |
| 2026-07-06 | iOS inspector is a touch dead-end | ENVIRONMENT | — | debug_assertions build config + touch platform not tested | fixed (uncommitted) |
| 2026-07-07 | iOS soft-keyboard Return never becomes `enter` | ENVIRONMENT | — | gpui-mobile insertText path never runs on host Rust | root-caused |
| 2026-07-07 | iOS dead Focus/Blur + no tracing sink swallows create error | ENVIRONMENT | — | platform-only event path; logging::init() desktop-only | root-caused |
| 2026-07-05 | GitHub page empty (FDW lazy writeback) | ENVIRONMENT | — | integration wiring absent in test | root-caused |
| 2026-07-05 | Page-click freeze (chained matview DDL hangs Turso actor) | ENVIRONMENT | — | matview boot ordering + actor threading not in headless wiring | root-caused |
| 2026-07-05 | dioxus: EventInfraModule alone = silent CRUD loss | ENVIRONMENT | — | embedder wiring divergence; keystone wires full_headless only | root-caused |
| 2026-07-05 | dioxus worker = tracing black hole | ENVIRONMENT | — | no subscriber in worker embedder | root-caused |
| 2026-07-05 | dioxus OPFS reload race | ENVIRONMENT | — | web persistence timing | open |
| 2026-07-05 | Stale row on navigation, ≥2.5s (cross-frontend) | ENVIRONMENT | ORACLE | settle-then-assert masks the transient window | open; needs mid-settle invariant |
| 2026-07-05 | dioxus block.name dynamic-schema view never materializes | ENVIRONMENT | — | seed/wiring divergence (org-seeded vs hand-seeded) | open |
| 2026-07-04 | inv-org-render-fixed-point flaky (silent 150ms settle budget) | ENVIRONMENT | — | settle budget « projection pass; silent timeout | FIXED (fail-loud 30s combined settle) |
| 2026-07-05 | Multi-second edit latency at vault scale (pass_ms ≈ 11.3 + 0.221×blocks) | ENVIRONMENT | ORACLE | no latency-budget invariant; test scale « vault scale | open; SLO defined 2026-07-07 |
| 2026-07-06 | iOS share/accept modal wider than screen (hardcoded 640px) | PERCEPTION | — | no pixel geometry at ReactiveEngine rung | fixed (uncommitted) |
| 2026-07-08 | Per-edit org writeback ran the O(N) recursive-CTE `get_blocks` 2×/edit (render + `materialize_images`), ~585ms@2k / ~4s@5k — breaches p95<200ms on the CRDT interactive path | ORACLE | ENVIRONMENT | no per-edit writeback SLO invariant / recursive-CTE-count assertion in keystone; recursive CTE is cheap at keystone's small N so wall never breaches | fixed (uncommitted): Tier-1 per-doc block cache + O(1) `block_raw` point-read + image-gated `materialize_images`; regression test `crates/holon-orgmode/tests/incremental_org_writeback_smoke.rs` asserts 0 recursive-CTE per content edit; keystone SLO invariant still open |
| 2026-07-09 | Advice weaver watched the `advice_suppressed` JUNCTION (`SELECT anchor_id, lesson_id`) — id-less rows hit the `watch_query` enrichment boundary (`row_id().expect`), panicking on a `tokio-rt-worker`; the dying task dropped 3 blocks from the shared keystone stream and dominated the shrinker | ENVIRONMENT | ORACLE | background-task panic isolation masks it: the deterministic `advice_step6` fired the panic (visible in its log) yet stayed GREEN because the spawned weaver task's death doesn't fail the test; the id-less-row path exists only in the live watch wiring, not the deterministic refresh path | FIXED (uncommitted): weaver now watches the entity-shaped canonical-read matview (`advice_watch_sql`, `lesson_id AS id`) instead of the junction — proven-incremental suppression anti-join (holon-advice `probe_outer_antijoin_is_incrementally_maintained`); retires the proxy trigger. Open gap: no assertion on background-weaver health / swallowed spawned-task panics |
| 2026-07-09 | iOS soft-keyboard Return inserts a literal `\n` instead of creating a block (add-block dead via the on-screen keyboard). A real `enter` keystroke DOES split/create (verified on sim via `type_text`/`send_raw_keystroke`), so only the soft-keyboard insertText:→enter translation was missing | ENVIRONMENT | — | keystone's raw-keystroke rung injects a synthetic `KeyDown "enter"` that bypasses gpui-mobile `handle_text_input`'s insertText: path; no soft-keyboard-faithful (insertText:) input rung exists in the harness | FIXED: gpui-mobile fork `68df9dd` routes `\n`/`\r` → `enter` (mirrors the fork's Backspace handling), pinned via Cargo.lock bump. Parity gap open: no insertText:-path rung in keystone |
| 2026-07-09 | iOS soft keyboard raises on editor focus then hides (~150ms, `KEYBOARD_HIDE_GRACE` / focus churn) — appears then dismisses rather than staying visible | PERCEPTION | — | keyboard show/hide is a device-visual timing property; no headless assertion; render-edge `editor_focus_gained/lost` + deferred-hide grace race | open |

Notes:
- The 2026-07-05 latency bug was originally classed PERCEPTION in the audit;
  reclassified ENVIRONMENT/ORACLE on 2026-07-07 when latency-over-budget was
  declared a formalizable bug (SLO above).
- The 2026-07-08 org-writeback latency escape is the read/render-side sibling of
  the 2026-07-05 projection-side latency escape: same class (an O(N)
  full-document re-materialization per single-block edit), now on the org
  writeback path. Primary ORACLE because the interaction (a content edit) and
  the failing code path (`on_block_changed → get_blocks`) both run in the
  keystone wiring — only a per-edit SLO/recursive-CTE-count invariant is
  missing; ENVIRONMENT secondary because the recursive CTE is milliseconds at
  the keystone's handful of blocks and only breaches wall-clock at vault scale.
  The added regression test asserts the structural proxy (zero recursive-CTE
  per content edit); a keystone p95<200ms writeback invariant remains the open
  gap.
- Successes are not escapes: the editor-caret divergence was *found by* the
  keystone oracle and does not belong here.
- The 2026-07-06 "iOS Focus/Blur never fire → tap doesn't move `focused_block`,
  keyboard/commit dead" premise (memory `ios-text-2-causes`) was VERIFIED FIXED
  on 2026-07-09 against the live iOS sim with real `idb` finger taps: a tap moves
  the editor authority, typing lands, and moving focus away commits — the
  Petri-net/`InputRouter` rework closed it. No longer an open escape.

## Deferred perf

- **Partitioned (per-anchor) top-K in Turso IVM.** The advice weaver applies its
  per-anchor top-K in Rust because Turso IVM has global `ORDER BY ... LIMIT` but
  no PARTITION-BY top-K operator, so the watched `advice_rule_{slug}` outer matview
  is per-anchor UNBOUNDED and each recompute is O(all-candidate-advice). Fine while
  advice sets are small. If it becomes a latency dominator (p95<200ms), push a
  partitioned top-K operator into the Turso IVM fork (`core/incremental/`) so
  K-per-anchor is maintained incrementally and the matview is bounded. Referenced
  from `holon_frontend::advice_weaver::recompute_sidecar` (TODO(partitioned-top-K)).
