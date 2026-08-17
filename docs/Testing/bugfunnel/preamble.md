# Bug Funnel — escape ledger

Every bug found OUTSIDE an automated test gets one row here, classified by the
`bug-gap-triage` skill (`.claude/skills/bug-gap-triage/SKILL.md`). The gap
distribution steers QA investment.

**Running distribution** (totals = archived baseline + sum of the increment log):

- ENVIRONMENT: 185
- COVERAGE: 126
- PERCEPTION: 72
- ORACLE: 75

Archived baseline (ENVIRONMENT 87 · COVERAGE 37 · PERCEPTION 35 · ORACLE 18 as of
2026-07-22): the per-bug increment log below starts at commit e70c3a9245f2, which split
the older single-line running distribution into the four counters above and carried its
values over unchanged. Everything counted before that date lives only in the Ledger table
— it was never restated as increment lines. Subtract this baseline before checking the
header against the log.

Increment log (append-only, NEWEST FIRST — each counted bug adds exactly one line here;
merge conflicts resolve by keeping both sides' lines and re-summing the totals ON TOP OF
the archived baseline):









- (+1 ORACLE +1 ENV 2026-07-28: main panel PERMANENTLY DROPS a row after an away-and-back refocus — the block exists everywhere in storage, its siblings render, and the panel is otherwise fully materialised, but it renders NO node and its `state_toggle` is unclickable. ORACLE primary: `inv-main-panel-rows-match-focus` was a SUBSET check only (rendered ⊆ allowed), structurally blind to a MISSING row — it passed 15/15 on runs where `block:parent` rendered nothing; strengthened to SET EQUALITY (`main_editable_descendants` must all render), which fires 3/3 on the committed repro and also reds the composed keystone on random walks. ENV secondary: the underlying defect is an UNPAIRED RETRACTION in the `watch_view_*` focus-descendants IVM delta stream — the NavigateHome prune emits `Deleted` for every row of the old focus subtree and the NavigateFocus-back emits `Created` for all but one, which is therefore never re-asserted; the dual of the 2026-07-27 retract-MISS row above (same view family, same vendored turso IVM). Every frontend stage is exonerated by three probes with ZERO divergences: generation guard 0 drops, `retain_keys` 0 evictions, and provider rows == driver `row_map` == `MutableTree` == rendered nodes at every `VecDiff` boundary; `inv-matview-consistent-with-recompute` was GREEN in the same run, so the matview CONTENT is right and only the DELTA lost the insert. Oracle strengthening + probes landed; the prod fix is turso-side and the repro case stays quarantined.)
- (+1 ENVIRONMENT +1 COVERAGE 2026-08-10 promotion send-back, found DURING the #79 fix rather than by the dogfood pass: the `#+TODO:` vocabulary never survives a real ingest (ENVIRONMENT primary — may render #68 and #79 inert in production), and the `state_toggle` CLICK still walks the widget ring while Cmd+Enter is now vocabulary-aware (COVERAGE primary). Only the LEADING token of this line counts toward ENVIRONMENT; the COVERAGE increment is carried on its own line below.)

<!--

This one-liner needs to be split up into multiple lines.
In one line it creates conflicts all the time.

-->

Gap definitions: **COVERAGE** = keystone couldn't generate the interaction ·
**ORACLE** = generatable but no invariant flags it · **ENVIRONMENT** = prod
wiring/timing/platform differs from test · **PERCEPTION** = visual/UX, no
formal invariant in current harness. Latency-over-budget (SLO: p95
interaction→projection-visible < 200ms) is ORACLE or ENVIRONMENT, never
PERCEPTION.
