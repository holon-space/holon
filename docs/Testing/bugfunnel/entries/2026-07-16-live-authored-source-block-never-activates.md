---
id: 2026-07-16-live-authored-source-block-never-activates
date: 2026-07-16
gap: PERCEPTION
secondary: ENVIRONMENT
retriaged_from: COVERAGE
status: OPEN
summary: >-
  Live-authored `holon_rule` source block never activates: created over MCP
  (correct content_type/source_language, lands in DB + discovery query
  matches), but no operate watcher starts and the rule never fires until app
  restart — despite rule discovery being a live CDC watch (`query_and_watch`
  subscription)
source_line: 823
---

## Bug

Live-authored `holon_rule` source block never activates: created over MCP
(correct content_type/source_language, lands in DB + discovery query
matches), but no operate watcher starts and the rule never fires until app
restart — despite rule discovery being a live CDC watch (`query_and_watch`
subscription)

## Missing piece

no test authors a rule at runtime through the op path and asserts watcher
spawn; CDC-watch → watcher-spawn path evidently not driven

## Remedy

ROOT-CAUSED 2026-07-17 (base integration `0e005fd1`) — the RUNTIME-AUTHORING
premise does NOT hold, same shape as the undo row 33 closure. The wiring IS
correct: `wiring.rs:407` → `action_watcher::start_action_watchers` →
`holon_rule_watcher::start_holon_rule_watchers`, a live `query_and_watch`
over `holon_rule_discovery.sql`, which `prepend_initial_data` also replays
as `Created` (`backend_engine.rs:648`). PROVEN by new prod-path test
`crates/holon-integration-tests/tests/holon_rule_runtime_discovery_prod_session.rs`
(`TestEnvironment::start_app`): a clock-subject operate rule authored AT
RUNTIME through the block `create` op is picked up by the live discovery
watcher and goes `RuleStatus::Active` within ~1s (Active is set ONLY on
`start_rule`'s success path immediately after "starting operate watcher",
`holon_rule_watcher.rs:189`). So the CDC-discovery → watcher-spawn path IS
driven at runtime. The dogfood's "never fires" is explained WITHOUT a broken
path: `start_rule` DESIGNEDLY does not start an operate watcher (and sets a
loud status or none) for rule shapes that are not clock-subject-with-emit —
a guard-only rule (no `emit`) returns silently as "not an operate rule", and
a block-subject or finer-than-day time trigger (e.g. the dogfood's
"now+2min") sets `RuleStatus::CompileError("block-subject operate rules are
not yet reactively wired … needs a non-chained-matview evaluator")`. The
RESTART observation ("0 operate watchers, not even seeded daily_journal") is
decoupled: it was taken against the dup-polluted DB produced by the
now-FIXED destructive-writeback + journals seed/file collision bugs (rows
23/25), and could not be reproduced from a clean boot. REMEDY: env-gap
regression test added (runtime-author path). ESCALATED FORK (feature design,
needs a ruling — do not guess): should Holon support sub-day / block-subject
time-triggered operate rules (the "+2min" shape)? That requires the
non-chained-matview evaluator ADR 0024 §7.2 flags as missing, not a wiring
fix. Residual OPEN: a restart-against-persisted-DB smoke test for
operate-watcher liveness (couldn't cheaply reproduce the dup-polluted
precondition post rows 23/25 fixes)
