---
id: 2026-08-06-banner-latches-once-per-document-instead
date: 2026-08-06
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  The `WritebackDegraded` banner latches once per DOCUMENT instead of once per
  stall EPISODE, so a second, different permanent stall on the same document
  is silently unwritten.
source_line: 771
---

## Bug

(inspection, while refuting a lane premise that the escalation counter never
advanced on an unchanged difference — it does, since Wave B′) **The
`WritebackDegraded` banner latches once per DOCUMENT instead of once per
stall EPISODE, so a second, different permanent stall on the same document
is silently unwritten.** `GateSkipState.escalated` clears only on a gate
PASS (`gate_skips.remove`, `file_sync_controller.rs:4401`) or on stream
Reset — never in the difference-CHANGED `else` arm (`:4456-4465`), which
already resets `consecutive`, `consecutive_any` and `resync_requested`.
Sequence: doc stalls on difference D1 → 8 blocked content-bearing edits →
banner naming D1; one member then folds, the difference becomes D2 and never
converges → `consecutive` restarts, climbs to 8 again, and the
`!entry.escalated` guard at `:4484` suppresses the disclosure. Edits keep
reaching Loro and SQL and keep NOT reaching disk, with the only user-visible
signal being a stale banner naming a difference that no longer exists.

## Root cause

secondary COVERAGE: found by INSPECTION while refuting a lane premise — the
write-back fold gate's `WritebackDegraded` disclosure latches ONCE PER
DOCUMENT, not once per stall episode. `GateSkipState.escalated`
(file_sync_controller.rs:437) is cleared only when the gate PASSES
(`gate_skips.remove`, :4401) or on stream Reset, never in the
difference-CHANGED `else` arm (:4456-4465) that already resets
`consecutive`, `consecutive_any` and `resync_requested`. So a document that
stalls on difference D1, discloses, then folds one member and stalls
PERMANENTLY on a different difference D2 climbs back to
GATE_SKIPS_BEFORE_DEGRADED=8 blocked content-bearing edits and says NOTHING
— unlimited unwritten edits behind one banner whose text names the now-stale
D1. Convergence does clear it, so the hole is exactly "stalled → moved →
stalled again, never converging". GAP: the covering PBT
(`a_permanently_incomplete_holder_escalates_to_degraded`) holds its
difference constant for all 10 edits, and its oracle asserts `raised.len()
== 1` — an assertion that is CORRECT for one episode and structurally cannot
see a second one; secondary COVERAGE because no test arranges a fold that
moves and then re-stalls. Not a keystone repro: the keystone settles to CDC
quiescence between transitions, so no permanently-short holder exists there
at all; pinned at the FileSyncController seam instead. FIXED same day:
`escalated` reset alongside `resync_requested` in the difference-changed
arm; `warned` deliberately left latched (WARN-storm suppression, carries no
stale identifying detail). Semantics ruling made AUTONOMOUSLY and
revertible: "disclose exactly once" becomes ONCE PER DIFFERENCE EPISODE —
the existing constant-difference test stays green untouched)

## Missing piece

The covering test
`sync_controller_mutation_pbt.rs::a_permanently_incomplete_holder_escalates_to_degraded`
holds its difference constant across all 10 edits and asserts `raised.len()
== 1` — an oracle that is correct for a single episode and structurally
blind to the second; missing piece = an episode-transition property (fold
one member mid-stall, assert a fresh disclosure naming the NEW difference).
Secondary COVERAGE: no test anywhere arranges a fold that partially moves
and then re-stalls. No keystone repro — it settles to CDC quiescence between
transitions, so a permanently-short holder never exists there; the
FileSyncController seam is the right altitude.

## Remedy

FIXED 2026-08-06: `escalated = false` in the difference-changed arm beside
`resync_requested`; `warned` left latched on purpose (per-skip WARN-storm
suppression, no stale identifying detail in it). Red-first:
`a_second_stall_episode_with_a_new_difference_discloses_again` failed with
`Raised: [<D1 banner>]` / left 1 right 2. Semantics ruling made AUTONOMOUSLY
and revertible: exactly-once becomes once-per-difference-episode; the
constant-difference test above stays green untouched.
