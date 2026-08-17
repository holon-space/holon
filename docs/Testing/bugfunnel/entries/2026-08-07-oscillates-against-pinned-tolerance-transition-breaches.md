---
id: 2026-08-07-oscillates-against-pinned-tolerance-transition-breaches
date: 2026-08-07
gap: ORACLE
secondary: COVERAGE
status: UNCLASSIFIED
summary: >-
  `OpenTabViaModifierClick.sql_reads` oscillates 23/24 against a pinned
  `expected` of 22 + tolerance 1, so the transition breaches on roughly two
  thirds of its occurrences.
source_line: 1180
---

## Bug

(found by a FLAKING GATE, not by any assertion — the PINNED budget fires on
a legitimate measurement) **`OpenTabViaModifierClick.sql_reads` oscillates
23/24 against a pinned `expected` of 22 + tolerance 1, so the transition
breaches on roughly two thirds of its occurrences.** Signature:
`[inv-sql-budget PINNED] OpenTabViaModifierClick.sql_reads: 24 exceeds
expected 22 + tolerance 1 = 23 (watches=0, docs=3) [PINNED ceilings gate RAW
reads; dedup was 14]`. Over the 43 OpenTab samples in one keystone run the
observed range is {23, 24} — **the pinned `expected` of 22 sits below the
observed FLOOR**, so the constant no longer describes the transition. Not a
bigger draw: at the identical reference state `b29/d3/w0/r5` the same run
measured 24 twenty-five times and 23 seven times.

## Root cause

secondary COVERAGE: found by a FLAKING GATE, not by any assertion — the
PINNED `OpenTabViaModifierClick.sql_reads` ceiling fires on a legitimate
measurement. Signature: `OpenTabViaModifierClick.sql_reads: 24 exceeds
expected 22 + tolerance 1 = 23 (watches=0, docs=3) [PINNED ceilings gate RAW
reads; dedup was 14]`. Across the 43 OpenTab samples in one keystone run the
observed range is {23, 24} — the pinned `expected` of 22 sits BELOW THE
OBSERVED FLOOR, so the constant no longer describes the transition and only
`CLICK_JITTER_TOLERANCE` keeps the low mode legal. It is genuinely
nondeterministic rather than a bigger draw: at the IDENTICAL reference state
`b29/d3/w0/r5` the same run measured 24 twenty-five times and 23 seven
times. TWO separate facts, both measured. (i) The tolerance's own rationale
is STALE. `CLICK_JITTER_TOLERANCE` (`crates/holon-pbt-core/src/budget.rs`)
documents the ±1 as `focus_roots`/`current_focus` coalescing "either 3 or 4
times", but in EVERY sample of both modes both statements fire exactly 4x,
while `dedup` moves 13→14 — so the extra read is a DISTINCT SQL text, not an
extra repeat of a coalescing read, and the documented mechanism cannot be
the one operating. (ii) The real ±1 is a BRANCH SPLIT nobody had modelled,
isolated from the `HOLON_PERF_DETAIL` dumps at matched state `b28/d4/w0/r1`:
`open_tab`'s insert-new-tab branch costs 23 reads / 13 unique (`SELECT
MAX(id) as max_id FROM navigation_history WHERE region = $region` plus
`INSERT INTO navigation_history`), its activate-an-already-open-row branch
costs 22 / 12 (the `navigation_cursor` LEFT JOIN `navigation_history` cursor
read instead, no `MAX(id)`, no insert). One `expected` models both branches,
so the counter is bimodal BY CONSTRUCTION and the tolerance is already spent
absorbing that. DELIBERATELY NOT RE-PINNED, per the constant's own standing
rule that "a breach means the click path grew … never nudged to make a run
pass": widening to 24 would spend the ±1 twice and leave the branch split
with no headroom, and the absolute level ALSO differs between corpora
(hand-authored `r=1` measures 22/23, keystone `r=4..5` measures 23/24, where
`r` = `main_rendered_block_ids().len()` — a term the pin's comment never
evaluated, having measured only documents, watches and first-visit at zero).
That corpus difference is CONFOUNDED (different vault shapes, d=3 vs d=4/5)
and is explicitly NOT attributed here, which is exactly why 24 would be the
ceiling of THIS corpus rather than of the transition. Escalated to Martin
with two options: split the budget per `open_tab` branch, which removes the
bimodality at its source, or ratify a count-blind known-red row — the
registry requires his ratification, so no row was added unilaterally.
**RULED (a) by Martin 2026-08-08 and FIXED** — the budget is now split per
`open_tab` branch at tolerance ZERO; see the ledger row for the shipped
shape and the measured per-branch distributions)

## Missing piece

Two measured facts. (i) The tolerance's own rationale is STALE:
`CLICK_JITTER_TOLERANCE` (`crates/holon-pbt-core/src/budget.rs`) attributes
the ±1 to `focus_roots`/`current_focus` coalescing "either 3 or 4 times",
but in every sample of BOTH modes both statements fire exactly 4x while
`dedup` moves 13→14 — the extra read is a DISTINCT SQL text, not an extra
repeat, so the documented mechanism is not the one operating. (ii) The real
±1 is an unmodelled BRANCH SPLIT, isolated from `HOLON_PERF_DETAIL` dumps at
matched state `b28/d4/w0/r1`: `open_tab`'s insert-new-tab branch costs 23
reads / 13 unique (`SELECT MAX(id) as max_id FROM navigation_history WHERE
region = $region` + `INSERT INTO navigation_history`) while its
activate-an-already-open-row branch costs 22 / 12 (the `navigation_cursor`
LEFT JOIN `navigation_history` cursor read instead — no `MAX(id)`, no
insert). ONE `expected` models both branches, so the counter is bimodal by
construction and the tolerance is already spent absorbing that. Secondary
COVERAGE: the 2026-08-03 corpus that set the pin never separated the two
branches, and its comment enumerates only documents / watches / first-visit
as terms measured at zero — never the branch, and never `r`
(`main_rendered_block_ids().len()`).

## Remedy

**DELIBERATELY NOT RE-PINNED**, per the constant's own standing rule that a
breach "must be re-measured deliberately, never nudged to make a run pass".
Widening to 24 would spend the ±1 twice and leave the branch split with no
headroom; and the absolute level also differs between corpora (hand-authored
`r=1` → 22/23, keystone `r=4..5` → 23/24), which is CONFOUNDED by vault
shape (d=3 vs d=4/5) and is explicitly NOT attributed here — so 24 would be
the ceiling of this corpus, not of the transition. ESCALATED to Martin with
two options: (a) split the budget per `open_tab` branch, which removes the
bimodality at its source and is the principled fix, or (b) ratify a
count-blind known-red row — proposed `Match pattern`
`OpenTabViaModifierClick\.sql_reads: [0-9]+ exceeds expected 22` (no
alternation, count-blind, per the registry's rules). No
`KeystoneKnownReds.md` row was added unilaterally: that registry requires
Martin's ratification and says "do not add a row to silence it". **RULING
(a) RATIFIED by Martin 2026-08-08 — FIXED (task #23).** Root cause as
diagnosed above: an UNMODELLED BRANCH SPLIT, not measurement noise. The
shipped shape: `UITabState.last_open_tab_activated` is recorded at apply
time (`crates/holon-integration-tests/src/pbt/ref_caps/nav.rs:285`) from the
same `already_open` predicate the reference's own dedup logic computes —
never sniffed from SQL text — is exposed as
`RefSqlCardinality::last_open_tab_activated`
(`crates/holon-pbt-core/src/capabilities.rs:2690`), and its ONLY read site
is the budget clause in
`crates/holon-integration-tests/src/pbt/transitions/open_tab_via_modifier_click.rs:153`,
which selects `OPEN_TAB_ACTIVATE_CLICK_RESOLVE_READS = 10` (⇒ 22 reads) or
`OPEN_TAB_INSERT_CLICK_RESOLVE_READS = 11` (⇒ 23) from
`crates/holon-pbt-core/src/budget.rs:86,110`. **Tolerance is ZERO for this
transition and must stay zero**: the branch delta IS 1, so any tolerance
lets the activate ceiling reach the insert cost and a backwards-wired flag
would sail through — `CLICK_JITTER_TOLERANCE` survives as `PinBlock`'s
alone, and its stale focus-coalescing rationale was corrected in the same
rev. Option (b) was NOT taken: no count-blind known-red row exists.
MEASURED, not assumed (`just hand-authored`, `HOLON_PERF_BUDGET=1`, 34-case
corpus): 7 OpenTab samples, `reads=22 (dedup 12)/22` ×1 (activate) and
`reads=23 (dedup 13)/23` ×6 (insert) — every sample EXACTLY on its own pin,
zero tolerance consumed, 9 passed / 0 failed. Both branches are
non-vacuously exercised by deterministic hand-authored cases
(`cmd-click-sidebar-opens-second-tab` = insert,
`cmd-click-same-row-twice-activates` = insert then activate). TEETH
re-proven this lane by inverting the branch flag: `[inv-sql-budget PINNED]
OpenTabViaModifierClick.sql_reads: 23 exceeds expected 22 + tolerance 0 = 22
(watches=0, docs=5) [PINNED ceilings gate RAW reads; dedup was 13]`, exit
101, file restored byte-identical (sha256 `d6cb8ce4…`). HONESTLY SCOPED: the
pins are calibrated on the `r=1` hand-authored corpus; the keystone `r=4..5`
regime measured one read higher before the change and that offset is still
UNATTRIBUTED and confounded with vault shape — at tolerance 0 a real offset
now reds loudly, which is the intended fail-loud outcome, and keystone-smoke
is NOT evidence for this transition (its observed runs draw zero OpenTab
transitions).
