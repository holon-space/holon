---
id: 2026-08-20-popup-setfield-e2e-interaction-never-closes
date: 2026-08-20
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Every popup-driven integration set_field left its e2e latency interaction
  pending until it expired as e2e_expired — WARN spam and an unmeasured,
  SLO-blind interaction — because no watched mirror delivered its target row.
---

## Bug

Found by dogfooding the popup-driven op_button set_field (the feature this lane
adds): every such dispatch logged, 64–145s later,

```
WARN holon_latency: interaction expired without a delivered row
     stage="e2e_expired" action=set_field
```

The interaction never closed, so it was both never measured (SLO-blind) and a
source of delayed WARN spam.

## Root cause

The dispatch site enrolls every id-carrying op as an
`Observable::BlockRow` interaction (`crates/holon-frontend/src/reactive.rs:3701`),
which closes only when a WATCHED `LiveData` mirror delivers a block row for the
target (`crates/holon-api/src/live_data.rs:509` → `rows_delivered`). A settings
`integration.set_field` writes the `.state.toml` authority; the
`IntegrationStateProjector` mirrors it into `integration_state`
(`crates/holon-app/src/integration_projection.rs`), which the Settings
`live_query` reads — but that projection never reported a delivery to the
correlator, so the interaction had no closing event.

Pinned empirically in the windowed harness
(`frontends/gpui/tests/settings_integrations_setfield_popup_windowed.rs`,
diagnostic run): after the popup dispatch flipped the mirror,
`latency_e2e::pending_targets()` still held `"integration:gcal"`; the mirror row's
`id` column was exactly `"integration:gcal"` (matches the enrolled target); and a
MANUAL `rows_delivered("probe", [("integration:gcal", BlockRow(None))])` closed
it immediately. So the match logic was correct — the real projection simply never
called `rows_delivered`.

## Missing piece

No invariant asserted that a settings `set_field` interaction CLOSES (rather than
expiring). The keystone/windowed rungs can generate the interaction, but nothing
judged its latency closure — an ORACLE gap (per `bug-gap-triage`, a latency
escape is ORACLE when the closing invariant does not exist/fire). The existing
latency rungs cover block-mirror and focus-root closures only; the integrations
mirror had no closure instrumentation at all.

## Remedy

**FIXED.** `IntegrationStateProjector::project()` now reports its projected rows
to the correlator via `holon_api::latency_e2e::rows_delivered("integration_state",
…BlockRow(None))` after each projection — the same mirror-apply closure point a
block mirror uses from `LiveData::subscribe`, and the projection IS the
projection-visible moment for the integrations surface. The interaction now
measures dispatch→projection and closes, eliminating the expiry.

Red-for-the-right-reason, then green
(`settings_integrations_setfield_popup_windowed.rs`, new assertion
`interaction_still_pending == false`):

```
RED: the popup-driven set_field e2e latency interaction for "integration:gcal"
     must CLOSE once the projection lands ... left pending it expires as
     `e2e_expired` ...
  test result: FAILED. 0 passed; 1 failed.

GREEN: test clicking_multi_param_set_field_opens_param_popup_then_dispatches ... ok
       test result: ok. 1 passed; 0 failed.
```
