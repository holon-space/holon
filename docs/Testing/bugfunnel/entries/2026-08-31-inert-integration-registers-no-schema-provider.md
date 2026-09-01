---
id: 2026-08-31-inert-integration-registers-no-schema-provider
date: 2026-08-31
gap: ENVIRONMENT
secondary: ORACLE
status: PARTIAL
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

Attribution, disclosure and the spawn-cause split have LANDED; the
assembled-page rung is still open.

**1. Attribution — done.** `IntegrationAttribution`
(`crates/holon-core/src/integration_attribution.rs`) maps every table an
integration's sidecar declares back to that integration, with the boot verdict.
It is declared BEFORE the connect attempt
(`crates/holon-app/src/mcp_integrations.rs`, `declare_entity_tables`), which is
what makes a FAILED connect attributable at all.

The link that made this possible: `MatviewManager::ensure_view` spelled the
cause into a fresh `anyhow!`, erasing the `StorageError::MissingDependencies`
variant. It now chains the source, and
`crates/holon-turso/tests/missing_deps_error_stays_typed.rs` pins it — reverting
that one `map_err` reproduces the exact error line above as the failure message.
Chaining moves the cause out of plain `Display`, so every consumer of that Err
had to move to `{:#}`: the watcher log, the live-query retry loop, and the
eager-backstop disclosure in `BackendEngine::query_and_watch` (that last one was
caught only by running the full `-p holon` suite, not the four-crate gate).

**2. Disclosure — a prop on a node every frontend renders, not a new kind.**
A block whose missing tables are FULLY explained by not-connected integrations
renders the `error` widget carrying the disclosure as its `message`, plus
`degraded_disclosure` and `integration` props, and logs WARN instead of ERROR —
so five ERROR lines become one calm named banner.

A first attempt used a bespoke `degraded` widget kind, with its own shadow and
GPUI builders, and was wrong. Those builders made it render on GPUI, which is
what hid the bug: `builder_registry!` registered the name wholesale, so the
interpreter built a real node with its props intact. It broke one layer down —
`to_view_kind` has no `"degraded"` arm, so the STATIC `ViewModel` became
`ViewKind::Empty`, `widget_name()` returned `None`, dioxus-web's
static-`ViewModel` dispatch took its `empty` arm and painted NOTHING (a visible
error became a blank region), and the headless snapshot read `"empty"` with no
message — invisible to every oracle. The name was also absent from
`TUI_SUPPORTED_WIDGETS`.

(Both builders are now deleted, so the name is merely UNREGISTERED, which fails
a third way — measured after the fact and pinned in the headless test: the
interpreter substitutes the placeholder text `[unknown: degraded]` and drops
the props. Either route loses the sentence; only the `ViewKind::Empty` route
above is what shipped.) `degraded_disclosure` follows the
existing `annotate_degraded` convention instead; GPUI styles the calmer colour
off it, and everything else keeps rendering the node it already knows. Pinned
headless by `crates/holon-frontend/tests/inert_integration_disclosure.rs` and
windowed by
`frontends/gpui/tests/inert_integration_disclosure_windowed.rs`, both of which
also pin the vanishing behaviour of a bespoke kind as the contrast.

**Partitioning is per table, not per failure.** The missing list is split:
tables owned by a settled-inert integration collapse to one disclosure EACH
(two dead integrations give two), while any table owned by a CONNECTED or still
`Pending` integration — or by nobody — keeps the whole failure loud and
attributed, whatever else shares the list. An earlier any-not-all predicate let
one dead integration swallow a genuine wiring failure into a calm WARN; that is
the "silently degrades to look fine" the error philosophy forbids.

**3. Spawn-failure classification — done, unix only.**
`crates/holon-mcp-client/src/command_resolution.rs` resolves a bare `command`
against the inherited `PATH` and then an explicit install list (`~/.cargo/bin`,
`~/.local/bin`, `/opt/homebrew/bin`, …) — an explicit list rather than a login
shell because it costs no subprocess per integration at boot and cannot be
broken by the user's shell rc. The list is APPENDED to the inherited `PATH`,
never substituted, so a binary on an exotic shell `PATH` resolves from where it
always did. This also fixes the Finder-launch case: a tool in `~/.cargo/bin`
now resolves under launchd's minimal `PATH`.

It is `#[cfg(unix)]`. On Windows the OS keeps resolving, because reproducing
`PATHEXT` here would be a second implementation of a rule we do not own — and
getting it wrong stops correctly-installed sidecars from starting at all.

Three causes are told apart, since each needs a different remedy: `binary not
found at <path>`, `binary '<name>' not found on PATH (searched …)`, and `found
at <path> but not executable` (a bundled sidecar that lost its exec bit — the
one case where "not found" would name the wrong cause).

### Still open

- **The assembled-page rung.** No test BOOTS with a dead sidecar and opens a
  page over its tables. What exists covers the partition and the collapse
  (`integration_attribution::tests`), the watcher's decision
  (`ui_watcher::tests`), the typed-error link (`missing_deps_error_stays_typed`),
  the node's survival to the web/headless layer (`inert_integration_disclosure`),
  and that the whole sentence paints in a real window
  (`inert_integration_disclosure_windowed`) — but not the assembly.
- **Oracle consequence of the new shape.** The disclosure is now `kind ==
  "error"`, which is exactly what **`inv-viewmodel-no-error-widgets`** filters
  on. So the headless fleet CAN finally see it — and a future harness that
  boots with a dead integration WILL turn that invariant red, because a
  disclosed banner is now indistinguishable from a render failure by kind
  alone. Such a rung must allow-list the disclosure signature (the message
  names the integration) rather than read it as a regression.
- **Do not cite `keystone-smoke` as coverage for this surface.**
  `inv-viewmodel-no-error-widgets` engages non-deterministically there —
  observed across runs as 17/17, 15/15, 36/36, 24/24, and `deselected`. The
  pinned coverage is
  `frontends/gpui/tests/inert_integration_disclosure_windowed.rs`.
- **The boot cause is in-memory only**, which is where the render path reads
  it. It is NOT persisted to `integration_state`; `config_status` was
  deliberately left alone (it is the configuration axis, not a cause field). A
  sidebar row that wants to show WHY needs a new `status_detail` column.
- **Key unification (residual).** `declare_entity_tables` keys tables on the
  config's provider key; `record_status` stamps the verdict by the same key,
  with a loud assert on the `NeedsAuth` branch where the connect result carries
  its own `provider_name`. If those ever diverge the tables would stay
  `Pending` forever — the assert makes that a crash rather than a silently
  unexplained page.
- **Runtime death after registration is NOT covered.** The DDL gate is
  registration-based, not existence-based, so this whole mechanism only fires
  when the sidecar was already dead at boot. An integration that registers its
  schema and THEN dies leaves the matview built and serving stale rows with no
  error and no disclosure at all — a separate, undisclosed-stale-data hole.

Original fix direction, for the record:

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
