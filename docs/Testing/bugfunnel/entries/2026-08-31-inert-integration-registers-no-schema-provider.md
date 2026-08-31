---
id: 2026-08-31-inert-integration-registers-no-schema-provider
date: 2026-08-31
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  An unavailable integration registers an inert operation provider but no
  schema provider, so every view over its tables fails hours later with a DDL
  error that names neither the integration nor the boot-time cause.
---

## Bug

Found by Martin opening the `ClaudeCode` page in the live GPUI app
(2026-08-31, `/tmp/holon-cold.log`). All five view blocks on the page failed
at once, 20:10:14, within 4 ms of each other:

| block | missing table |
|---|---|
| `block:cc-sessions-chat` | `cc_session` |
| `block:cc-tasks` | `cc_task` |
| `block:cc-projects` | `cc_project` |
| `block:cc-sessions` | `cc_session` |
| `block:cc-conversation` | `cc_message` |

Each as:

```
ERROR [UiWatcher] render_entity('block:cc-sessions') failed: Failed to create
  materialized view watch_view_2ed15df44eed46a0: CREATE MATERIALIZED VIEW ...
  FROM cc_session WHERE message_count > 0 — cause: missing dependencies
  ["cc_session"] — no schema provider registers them.
```

These are 5 of the 20 ERROR-level lines in the whole 18-hour session. They are
one-per-page-element amplification of a single boot failure, not five bugs.

## Root cause

When an MCP integration cannot connect, `mcp_integrations.rs:625-635`
registers an `EmptyOperationProvider` so operation dispatch stays total. It
registers **nothing on the schema side** — the entity tables the sidecar
declares (`cc_session`, `cc_message`, `cc_task`, `cc_project`) never get a
schema provider.

Six hours later the DDL gate at `crates/holon-turso/src/turso.rs:3262` sees a
matview naming resources nobody registers and fails it rather than waiting
(correctly — waiting would hang; see the `turso-chained-matview-hang` skill).
The error text it composes
(`crates/holon-core/src/storage/types.rs:53-57`) states the missing table
names and nothing else.

The real cause WAS disclosed loudly at boot, on the degraded bus and in the
log. But nothing carries that attribution forward to the moment a view over
those tables fails. The user is shown five internal identifiers and a matview
hash; the sentence "the `claude-history` integration failed to start" appears
nowhere near the failure.

## Missing piece

There is no link from an unavailable integration to the tables it owns. Two
consequences, neither covered by any test:

1. **Attribution.** The DDL failure cannot say "table `cc_session` belongs to
   integration `claude-history`, which is Unavailable — cause: <boot error>".
2. **Disclosure.** Per the repo's error philosophy the page should render a
   visible degraded banner naming the integration, not five raw internal
   errors. The `IntegrationStatus::Unavailable` row is already recorded
   (`integration_projection`); no view consults it.

No PBT covers a page whose views read a NOT-connected integration's tables.
The keystone catalog builds views over vault blocks, where the schema
provider always exists, so this wiring never runs in the test environment —
an ENVIRONMENT gap. The ORACLE half: even in the windowed harness no
invariant asserts "a failed view names a cause the user can act on".

## Remedy

Open. Fix direction, in order of value:

1. Have the integration registry publish the entity-prefix → integration
   mapping (`entity_prefix: "cc_"` is already in the sidecar) so the DDL
   failure path at `types.rs:53-57` can resolve `cc_session` back to
   `claude-history` and append its recorded status and boot cause.
2. Render the degraded state: a view whose tables belong to an Unavailable
   integration should paint a disclosed banner ("claude-history not
   connected"), not an error node per block.
3. Add a keystone/GPUI rung that boots with a deliberately dead sidecar and
   opens a page reading its tables — asserting the rendered output names the
   integration. That test would also have caught
   `2026-08-31-bundled-sidecar-hardcodes-developer-local-binary-path`, which
   is the trigger here.
