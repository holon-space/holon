---
id: 2026-08-17-cold-boot-initial-scan-feed-did-not-converge
date: 2026-08-17
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  Cold boot against Martin's real vault: 27 of 1930 expected blocks never
  converged into the feed within the 30s stall window.
---

## Bug

Found by log analysis of `/private/tmp/holon-cold.log` (2026-08-17 cold boot
against `/Users/martin/Workspaces/pkm/holon-pkm`, ~1930 blocks). Line 2246:
`[finish_initial_scan] block feed did not converge — no progress for 30000ms
with 27 of 1930 expected id(s) still missing — projection/CDC stalled during
the initial scan`. Boot did NOT crash — the error is caught and pushed to
`di.rs`'s `failures` vec (`[OrgMode] initial-scan feed convergence failed`,
line 1071-1073), and the app continued to a working MCP server. But 27
blocks from the real vault were never indexed into the projection this boot.

## Root cause

`FileSyncController::finish_initial_scan`
(`crates/holon-filesystem/src/file_sync_controller.rs:904-943`) waits for the
CDC/matview feed to catch up on every id the initial scan expects, via
`wait_for_feed_progress` — progress-grounded (no fixed wall-clock cap, only a
per-slice stall detector), specifically designed per its own doc comment to
handle "a real vault cold boot legitimately takes minutes" (BugFunnel
2026-07-12). It bails loudly only when a full `stall_ms` (30s) window passes
with ZERO new ids landing — a genuine quiescent-but-incomplete feed, not a
slow-but-alive one. 27 ids stayed missing after quiescence.

## Missing piece

ENVIRONMENT primary: this is a real-vault-SCALE phenomenon (~1930 blocks);
several prior entries document scale-specific cold-boot defects (see
`2026-07-12-cold-boot-over-martin-real-vault`,
`2026-07-16-vault-scale-latency-slo-catastrophically-breached`,
`2026-08-02-cold-boot-copy-real-vault-1001`) — this is a NEW instance of that
family, not a duplicate (different missing-id count, different session), but
part of an established pattern of real-vault-scale ingest degradation that no
smaller keystone corpus reproduces. COVERAGE secondary: the keystone's
composed PBT boots against a small seeded/hand-authored corpus, never a
~1930-block real-vault-scale tree, so this stall shape is structurally
ungeneratable today — there is no rung that drives ingest at this scale to
find which 27-of-N blocks class of content triggers non-convergence.

## Remedy

NOT FIXED. The 27 missing ids were not identified from the log alone (the
log doesn't name which ids stalled, only the count) — a repro would need to
capture the specific ids on a future occurrence (log the actual missing ids,
not just the count, at `file_sync_controller.rs:936-939`) to localize which
content/matview shape stalls convergence. Disclosure already correct
(non-fatal, loud error, boot continues) — the gap is diagnosability and
real-vault-scale coverage, not silence.
