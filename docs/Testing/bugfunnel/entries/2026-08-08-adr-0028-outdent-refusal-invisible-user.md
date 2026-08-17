---
id: 2026-08-08-adr-0028-outdent-refusal-invisible-user
date: 2026-08-08
gap: PERCEPTION
secondary: ORACLE
status: OPEN
summary: >-
  The ADR 0028 D1 outdent refusal is invisible to the user and recorded as an
  engine ERROR.
source_line: 766
---

## Bug

(dogfood-explorer gate pass) **The ADR 0028 D1 outdent refusal is invisible
to the user and recorded as an engine ERROR.** shift+tab on a direct page
child is correct in substance — structure unchanged, no crash, five log
lines, sample correctly retired as `reason="op refused or failed — no write,
nothing to measure"` — but nothing appears in the UI, and the excellent
explanation is logged at ERROR from
`holon_frontend::reactive::dispatch_intent_awaitable`.

## Root cause

dogfood-explorer gate pass — **the ADR 0028 D1 outdent refusal is invisible
to the user and recorded as an engine ERROR**. shift+tab on a direct child
of a page behaves correctly in every substantive way: structure unchanged,
no crash, five log lines for the whole gesture, and the latency correlator
retires the sample properly (`stage="e2e_retired" … reason="op refused or
failed — no write, nothing to measure"`). But nothing appears in the UI — no
toast, no shake, no disabled cue — so a very common keystroke simply does
nothing, and the excellent explanation (`Cannot outdent a direct child of a
page … would escape its page container (ADR 0028 D1). Move it elsewhere
instead.`) is logged where only a developer sees it, at ERROR, from
`holon_frontend::reactive::dispatch_intent_awaitable`. Secondary ORACLE: a
DESIGNED refusal logged at ERROR reds `inv-no-observed-errors` for correct
behaviour — the same severity-vs-correctness call the 2026-08-08 stale-home
retire lane made the other way (WARN, deliberately). Evidence:
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§3)

## Missing piece

No invariant can demand a toast. The secondary is the severity call: a
DESIGNED refusal at ERROR reds `inv-no-observed-errors` for correct
behaviour — the opposite of the WARN choice the same-day stale-home retire
lane made deliberately.

## Remedy

**OPEN — reported, not fixed.** The refusal logic itself is GREEN and was
the named target of this pass. Evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§3.
