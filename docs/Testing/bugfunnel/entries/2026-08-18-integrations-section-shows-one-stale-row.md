---
id: 2026-08-18-integrations-section-shows-one-stale-row
date: 2026-08-18
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Every integration's boot status was refused and lost because the connect
  registry ran before the projector created the mirror rows, leaving every
  integration reading Pending forever.
---

## Bug

Found by Martin dogfooding the live GPUI app (cold boot 2026-08-17 23:42,
`/tmp/holon-cold.log`, screenshot) on a fresh DB created via the reseed gesture,
with gcal, gmail, todoist and claude-history all enabled by state files.

The left-sidebar Integrations section showed a single row —
`claude-history — Pending` — with gcal, gmail and todoist absent, and it still
read `Pending` after more than five minutes even though the log showed
claude-history connected.

This is a dogfood escape on code this lane landed hours earlier (entry
`2026-08-18-integrations-discovery-section-lists-only-orgmode`).

## Root cause

**Boot ordering.** The integration registry's connect loop records each
provider's outcome through `set_integration_status`
(`crates/holon-app/src/mcp_integrations.rs`), which refuses a provider that has
no enabled row in the mirror. That refusal is correct in isolation — a status
for a provider the store never enabled IS a wiring bug — but at that moment the
rows genuinely did not exist: the registry factory is resolved inside
`di.factory.FrontendSession.resolve_engine`, while `IntegrationStateProjector`,
which creates the rows, ran later in the same session factory
(`crates/holon-app/src/wiring.rs`, after `seed_default_layout` and
`flush_seed_layout`).

Nothing retries a refused status, so all four outcomes were discarded and every
integration was left at the `Pending` its row is born with.

The log is unambiguous — four refusals, 23:42:35 to 23:42:37, one per provider:

```
[McpIntegrationsModule] Could not record boot status for 'gcal' — the Integrations
section will show it as Pending: integration 'gcal' has no enabled row in the
integration_state mirror, so its boot status (Connected) cannot be recorded — the
connect registry and the enablement store have diverged
```

claude-history=Connected, gcal=Connected, gmail=Unavailable, todoist=Connected —
all lost. The span order in the same log confirms the mechanism:
`resolve_engine` → `seed_default_layout` → `flush_seed_layout` →
`start_action_watchers`, with the registry's refusals inside the first and the
projector between the third and fourth.

**A second defect, contributing:** the projector logged NOTHING on success, so
the boot log carried not one line from it while the section was visibly wrong.
Whether it had run at all could only be inferred from the rows.

### Two hypotheses tested and REFUTED

The missing rows were initially suspected to be a change-propagation gap — that
raw `execute_values` writes to a native table emit no notification a
`live_query` would observe, so the section latched one snapshot. Two rungs were
written to settle it empirically and **both pass**
(`crates/holon-integration-tests/tests/integration_state_section_refreshes.rs`):
a row written after the watch is live reaches the section, and a watch that goes
live on an EMPTY mirror still receives every later row. Writes to
`integration_state` do propagate.

A short mirror was also suspected. Refuted:
`integration_state_boot_population.rs` shows a real boot leaving one row per
bundled provider, and the boot-status rung below shows both enabled providers
present with `enabled = 1`.

So the `Pending` half is fully explained and fixed; the missing-rows half is
**not reproducible** in any harness reachable here — see Remedy.

## Missing piece

**No test booted the production wiring with real `.state.toml` files present.**
The projection tests construct `IntegrationStateProjector` directly and call
`project()` themselves, so they can never observe boot ORDER; and with no
provider enabled, the registry never connects and never records a status, so the
registry↔projector interaction was entirely unexercised. The refusal guard was
tested in isolation (`a_status_for_a_disabled_integration_is_refused`) — proving
it fires — but nothing checked that production never puts it in a position to
fire.

The rung that would have caught it, and now exists:
`crates/holon-integration-tests/tests/integration_state_boot_records_status.rs`
— plant state files in the config dir's `integrations/` BEFORE `start_app`, boot
the real wiring, then assert no enabled provider is still `Pending`.

Secondary COVERAGE: the GPUI seeded-sidebar test cannot see any of this either,
because `TestServices` fakes `watch_query_live` with canned static rows
(`frontends/gpui/tests/support/mod.rs`) — it proves the section RENDERS rows,
never that it RECEIVES the right ones.

## Decisive evidence — the live database

Read directly out of Martin's `~/.config/holon/holon.db` (copied, then
`sqlite3 .recover`, because stock sqlite3 cannot parse Turso's matview DDL):

```
integration:claude-history  claude-history   enabled=1  status=Pending  2026-08-17 23:42:37
integration:gcal            gcal             enabled=1  status=Pending  2026-08-17 23:42:37
integration:gmail           gmail            enabled=1  status=Pending  2026-08-17 23:42:37
integration:jsonplaceholder jsonplaceholder  enabled=0  status=Pending  2026-08-17 23:42:37
integration:todoist         todoist          enabled=1  status=Pending  2026-08-17 23:42:37
```

This settles three things at once:

1. **The projector RAN** — all five bundled providers, correct
   `integration:<provider>` ids, written at 23:42:37, seconds after the last
   refused status write (todoist, 23:42:37.472). The "zero projector log lines"
   observation was an artifact of the projector logging nothing on success, not
   evidence it never ran.
2. **The `Pending` half is exactly this bug** — every status is the birth value,
   because all four registry writes were refused.
3. **The missing-rows half is NOT a data defect.** Four rows satisfy the
   section's `WHERE enabled = 1`, and the seeded query in `block_raw` is the
   correct one. The section had four rows to show and showed one — which makes
   it a rendering/delivery defect above the mirror, recorded separately as
   `2026-08-18-integrations-section-renders-one-of-four-rows`.

## Remedy

**Fixed (the `Pending` half).** The registry factory now populates the mirror
before its connect loop, in the same sequence, so the ordering is structural
rather than incidental (`crates/holon-app/src/mcp_integrations.rs`). The session
factory's call remains — it installs the change watchers and covers a container
that never resolves the registry. The projector also logs one line naming how
many providers it projected and how many are enabled.

Red-for-the-right-reason, then green:

```
every enabled integration must carry the boot status the registry computed, but
[("gcal", "Pending"), ("todoist", "Pending")] are still Pending — the registry ran
before the projector created their rows, its status writes were refused, and
nothing retried.
```

The missing-rows half is a DIFFERENT escape — the mirror held all four rows —
and is filed separately as
`2026-08-18-integrations-section-renders-one-of-four-rows`.
