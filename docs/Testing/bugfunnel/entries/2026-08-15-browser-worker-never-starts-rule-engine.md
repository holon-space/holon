---
id: 2026-08-15-browser-worker-never-starts-rule-engine
date: 2026-08-15
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The browser worker never starts the rule engine, so the seeded
  `daily_journal` rule can never fire and the Journals view has no day page,
  on any visit, forever.
source_line: 696
---

## Bug

(task-#38 web lane; found by USER REPORT — Martin: "holon.space offers no
way to interact" — and localised by driving the live site headlessly) **The
browser worker never starts the rule engine, so the seeded `daily_journal`
rule can never fire and the Journals view has no day page, on any visit,
forever.** The worker hand-mirrors `holon-app/src/wiring.rs` inside
`build_engine_state` (`frontends/holon-worker/src/lib.rs:336-339`); that
mirror seeds the layout but omits the watcher startup.
`start_action_watchers` has exactly one caller repo-wide — native
`wiring.rs:511`, under `#[cfg(not(target_arch = "wasm32"))]` — and no
`RuleWatcher`/`ActionWatcher` reference exists anywhere under
`frontends/holon-worker/src/`. The rule block, the `journal_day_pages`
matview and the feed query were all correct; only the actor was missing.
Second silent drop by the same hand-rolled mirror (the block
`OperationProvider` at `lib.rs:300-310` was the first).

## Root cause

task-#38 web lane, found by USER REPORT (Martin: "holon.space offers no way
to interact") and localised by driving the live site headlessly: **the
browser worker never starts the rule engine, so the seeded `daily_journal`
rule can never fire and the Journals view has no day page — on ANY visit,
forever.** Root cause is the SECOND drift of one class: the worker
hand-mirrors `holon-app/src/wiring.rs` inside `build_engine_state`
(`frontends/holon-worker/src/lib.rs:336-339` says so explicitly), and that
mirror seeds the layout (`lib.rs:331-334`) but omits the watcher startup.
`start_action_watchers` (`crates/holon/src/api/action_watcher.rs:50`, which
also spawns `holon_rule_watcher::start_holon_rule_watchers`) has exactly ONE
caller repo-wide — native `wiring.rs:511`, itself under
`#[cfg(not(target_arch = "wasm32"))]` at `wiring.rs:508` — and `grep -rn
"RuleWatcher\|ActionWatcher" frontends/holon-worker/src/` returns ZERO hits.
The rule BLOCK is seeded correctly (`seed.rs:258-260` shares the spec via
`holon_frontend::journals_page_blocks().chain(journals_auto_create_blocks())`,
mirroring `assets/default/Journals.org`), the `journal_day_pages` matview is
created fine at boot, and the feed query is wired — nothing was missing
except an actor to fire the rule. The UI stated the root cause verbatim and
was not believed for weeks: the rule card renders `last fired: -`. NOTE this
is the same wiring-mirror class already documented one screen earlier in the
same file (`lib.rs:300-310`, the block `OperationProvider` missing because
"the worker doesn't load [turso_seams.rs]") — so the hand-rolled mirror has
now silently dropped two different pieces of production wiring. ENVIRONMENT
on the skill's own tiebreak, and not a close call: booting and viewing
Journals is trivially generatable headlessly and the keystone's wiring DOES
start watchers, so no invariant could ever have gone red — the failing code
path (`build_engine_state`, wasm-only, hand-rolled) does not exist in the
keystone's wiring at all. Deeper structural cause, and the reason it
survived: NO GATE boots the wasm worker far enough to notice a never-firing
rule engine — `just check-worker-wasm` compiles it and runs 5 serde unit
tests, and the Playwright `worker-smoke` spec does not assert any
rule-derived content. Sibling of the D1/D2 web-lane escapes (task #33).
FIXED in this lane, 8 lines: `start_action_watchers(engine.clone())` inside
`runtime.block_on` immediately after `seed_default_layout`. Feasibility was
verified rather than assumed — `action_watcher`/`holon_rule_watcher` carry
NO `cfg(target_arch)` guards and are unconditionally public
(`api/mod.rs:20,35`), and `spawn_actor` (`crates/holon/src/util.rs:27,35`)
branches on `all(wasm32, target_os="unknown")` while the worker is
wasm32-wasip1-threads i.e. `target_os="wasi"`, so it takes the tokio arm and
is driven by the worker's existing JS `engineTick` pump. PROVEN RED/GREEN AT
RUNTIME on the real browser stack, not merely compiled: worker wasm built
BOTH ways via the canonical napi path (`--features browser --profile
release-official`, EXIT=0 both), same `dist/`, same `serve.mjs`, fresh
Playwright context each run, only the wasm differing — WITHOUT the fix,
clicking Journals shows only "Journal Auto-Create", no `2026-08-15` day
page, zero action_watcher spans; WITH it, `2026-08-15` renders above the
rule block and the watcher spans appear. Evidence
`lane-logs/t38b-local-RED-nofix.txt`, `t38b-local-GREEN-daypage.txt`, driver
`t38b-probe3.mjs`, report `t38b-seed-report.md`. `just check-worker-wasm`
green with the fix. STILL OPEN, deliberately not papered over: no PBT covers
this yet — a covering test must boot the wasm worker and assert rule-derived
content, which no current harness rung does, and the holon-feature PBT
question is escalated to Martin separately; and Martin's ORIGINAL "no way to
interact" symptom is NOT explained by this fix — a fresh profile on the
deployed build already renders a working interactive outliner, so a stale
service-worker bundle or a sticky bad OPFS DB (`seed.rs:44-51` early-returns
forever once `block:root-layout` exists) remains unexcluded.)

## Missing piece

Booting and viewing Journals is trivially generatable headlessly and the
keystone's wiring DOES start watchers, so no invariant could go red — the
failing path (`build_engine_state`, wasm-only, hand-rolled) does not exist
in the keystone's wiring at all. Structurally, NO gate boots the wasm worker
far enough to notice a never-firing rule engine: `just check-worker-wasm`
compiles it and runs 5 serde unit tests, and the Playwright `worker-smoke`
spec asserts no rule-derived content.

## Remedy

FIXED, 8 lines: `start_action_watchers(engine.clone())` inside
`runtime.block_on` right after `seed_default_layout`. Feasibility verified,
not assumed — the watcher modules carry no `cfg(target_arch)` guards, and
`spawn_actor` branches on `all(wasm32, target_os="unknown")` while the
worker is wasm32-wasip1-threads (`target_os="wasi"`), so it takes the tokio
arm driven by the existing JS `engineTick` pump. PROVEN RED/GREEN on the
real browser stack with only the wasm differing
(`lane-logs/t38b-local-RED-nofix.txt`, `t38b-local-GREEN-daypage.txt`,
driver `t38b-probe3.mjs`). STILL OPEN: no PBT covers it — a covering test
must boot the wasm worker and assert rule-derived content, which no harness
rung does; escalated to Martin. Martin's original "no way to interact"
symptom is NOT explained by this fix and remains open.
