---
id: 2026-08-08-e2e-samples-never-close-navigate-own
date: 2026-08-08
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `navigate` e2e samples never close on the navigate's own delivery; an
  unrelated later row batch closes them and fires a FALSE user-facing `ORACLE
  VIOLATION: [latency-slo]` banner
source_line: 763
---

## Bug

(dogfood-explorer gate pass) **`navigate` e2e samples never close on the
navigate's own delivery; an unrelated later row batch closes them and fires
a FALSE user-facing `ORACLE VIOLATION: [latency-slo]` banner** — four in one
~15-minute session (11982ms, 10471ms, 468ms navigate, 26375ms split_block)
plus two `e2e_expired` at waited_ms≈56000. The navigate delivers on
`source="focus_roots"`; the correlator closes only on `source="block"`.
Twice-run idle probes (30s, 40s) produced neither an e2e nor an expiry, so
the reaper is event-driven and an idle app accumulates open samples.

## Root cause

dogfood-explorer gate pass — **`navigate` e2e samples never close on the
navigate's own delivery; a foreign row batch closes them and fires a FALSE
user-facing SLO violation**. The navigate delivers on `source="focus_roots"`
(+4ms, and describe_ui measures painted geometry for the new page at
+100ms); the correlator only closes on `source="block"`, so the entry
survives until some unrelated later block-row batch closes it and bills
every intervening idle second to the navigate. Four false `ORACLE VIOLATION:
[latency-slo]` banners in one ~15-minute session (11982ms, 10471ms, 468ms
navigate; 26375ms split_block), plus two very late `e2e_expired` at
waited_ms=56693/56124 for the same action. Twice-run controlled probe: click
a sidebar page then sit idle — 30s and 40s, NO e2e and NO e2e_expired either
time, so the reaper is event-driven and an idle app silently accumulates
open samples. This is the sibling of the same-day tokenless-op correlator
row: that lane fixed structural ops (split 25ms, join 32ms, indent 18ms,
outdent 17ms all close promptly and correctly here, and a refused outdent
retires with `reason="op refused or failed — no write, nothing to
measure"`), but `navigate` was not covered and now cries wolf loudly enough
to mask a real regression. Secondary in the same stream: one 21-char type
burst emitted ~14 separate `e2e action=set_field` lines for one block from
several subscribe_actor spans (44/48/69/71/79ms inside a 60ms window), so
any p95 off this stream double-counts single deliveries. ORACLE per the
latency carve-out — the interaction is trivially generatable, the correlator
is byte-identical in test and prod, and no invariant asserts that a
dispatched interaction yields exactly one e2e sample. Evidence:
`docs/Testing/fixture-logs-2026-08-08/dogfood-navigate-e2e-never-closes.txt`.
Disclosed caveat: the app reported `[interaction-pump] WINDOW-INACTIVE`
throughout, so the absolute wall-clock figures are not paint latency — the
missing correlation is a source-name mismatch visible in the event stream
itself and does not depend on them)

## Missing piece

Sibling of the same-day tokenless-op correlator fix, which covered
split/indent/outdent/join (all measured healthy here: 25/18/17/32ms) but not
`navigate`. Nothing asserts that a dispatched interaction yields exactly one
e2e sample; a 21-char type burst likewise emitted ~14 `set_field` e2e lines
for one block, so any p95 off this stream double-counts.

## Remedy

**FIXED 2026-08-08 (task #13, `crates/holon-api/src/latency_e2e.rs` +
`live_data.rs`).** ROOT CAUSE, one level below the reported source-name
mismatch: the `focus_roots` matview projects `(region, root_id, added_ts,
history_id)` — no `id`, no `parent_id` — and `LiveData::subscribe` built its
delivery list by reading exactly those two columns, so a navigation's own
batch produced ZERO delivery pairs and `rows_delivered` early-returned. The
entry stayed open until a later edit under the page delivered the child's
`parent_id = page_id`, which the tokenless close rule accepted. FIX: a new
`Observable` (`BlockRow(Option<WriteSeq>)` \ | `FocusRoot`) is carried on
BOTH ends — what an interaction waits to see, and what a batch delivered —
and matching is scoped per (target, kind), so a block row neither closes NOR
supersedes a pending `navigate`, and vice versa. `touched_entities` reads a
focus_roots batch by its own `root_id`, keyed on the mirror's source name
(`FOCUS_ROOTS_SOURCE`, used at the `subscribe` call site) rather than on the
presence of a `root_id` column — `blocks_with_paths` has one meaning "root
ancestor". The four dispatch seams name their observable;
`navigation.focus`/`open_tab` pass `FocusRoot`. Red-first, 3 reds all
failing for the mechanism: `[("navigate", 12000)]` closed by a foreign child
row (the false-banner value itself), the focus_roots delivery list `left:
[]`, and a pending navigate DROPPED as superseded by a same-id write. Green
21/21 in the module, 455/455 holon-api lib; all 16 task-#10 pins
behaviourally unchanged (only their navigate fixtures moved to focus-root
deliveries). `navigation_closes_on_page_child_row` was RENAMED to
`navigation_closes_on_its_own_focus_root_delivery` — the old name asserted
the defect as intended behaviour. Also: `rows_delivered` now prunes before
matching, so an expiry is disclosed on the next delivered batch rather than
the next dispatch (the probes' waited_ms≈56000); a fully idle app still runs
no reaper, but a lingering navigate entry is now inert (only a focus-root
delivery for the SAME page can close it, and newest-wins supersedes it), so
the residual is disclosure latency, not a wrong sample. NOT fixed here,
still open: the ~14-`set_field`-lines-per-type-burst double-count. Evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-navigate-e2e-never-closes.txt`,
red/green
`docs/Testing/fixture-logs-2026-08-08/task13-navigate-observable-red-green.txt`.
Disclosed caveat from the report: `[interaction-pump] WINDOW-INACTIVE`
throughout, so the wall-clock figures are not paint latency — the missing
correlation was visible in the event stream itself and does not depend on
them.
