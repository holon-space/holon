---
id: 2026-08-04-degraded-mode-disclosure-channel-wired-shipped
date: 2026-08-04
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The degraded-mode disclosure channel is not wired in the shipped GPUI
  container, and the app says so verbatim at boot:
source_line: 780
---

## Bug

(dogfood, boot log, real-vault copy) **The degraded-mode disclosure channel
is not wired in the shipped GPUI container, and the app says so verbatim at
boot:** `[McpIntegrationsModule] No DegradedSignalBus in this container
((ServiceNotProvided) - No provider registered for type:
Arc<holon_loro::degraded_signal_bus::DegradedSignalBus>…) — integration
connect failures will be LOG-ONLY and their pages will render blank with no
banner`, emitted under `di.factory.FrontendSession.resolve_engine`. Every
MCP-integration connect failure therefore degrades SILENTLY to a blank page
— priority 4 of the project's own error-handling order ("silently degrades
to look 'fine' — never do this"). This is the mechanism behind several
previously-filed "renders blank with no banner" observations rather than a
new symptom of each: the bus that would raise them does not exist in this
container.

## Missing piece

The wiring divergence is between DI containers, and no test asserts that the
container the SHIPPED binary builds provides every service that disclosure
depends on. Missing piece = a boot-time assertion (or a DI-container test)
that `FrontendSession`'s container resolves `DegradedSignalBus`, failing
loud at startup rather than logging a warning and continuing in a
silently-undisclosed mode. The module already detects the condition — it
just proceeds.

## Remedy

FIXED 2026-08-04 (disclosure-wiring lane) — TWO-PART, because part one alone
left the symptom unchanged. **(1) Registration.** `Arc<DegradedSignalBus>`
moved out of `LoroModule::register_subtree_share` (where it was reachable
only when `crdt.enabled`, and `HolonConfig::crdt_enabled()` is
`unwrap_or(false)` — so the SHIPPED default had no bus at all) into the
composition root, `holon-app` `add_frontend`
(`crates/holon-app/src/wiring.rs:118`), registered unconditionally before
any conditional module. It is a plain `tokio::broadcast` channel with no
Loro/iroh dependency, so mode has no say in whether it exists. Absence is
now a HARD BOOT ERROR: `McpIntegrationsModule` panics with the same
diagnostic instead of warn-and-continue
(`crates/holon-app/src/mcp_integrations.rs:182`), and `degraded_bus` is
`Arc<…>` rather than `Option`, so its three disclosure sites are
unconditional. A SECOND instance of the same hole was found and closed while
fixing: the `post_ready` org-initial-scan banner was also inside `if let
Ok(bus) = try_resolve_async(…)` (`wiring.rs:548`), so a failed vault ingest
was silently un-bannered in the shipped default too. **(2) Subscriber.**
Registration only creates a channel — adversarial verification established
that after (1) the shipped SqlOnly build had the bus with ZERO subscribers,
i.e. the observable symptom was literally unchanged.
`share_ui::spawn_degraded_bus_bridge` now takes `Arc<DegradedSignalBus>`
instead of `Arc<LoroShareBackend>` (`frontends/gpui/src/share_ui.rs:412`)
and subscribes SYNCHRONOUSLY before its pump task is scheduled; its spawn in
`launch_holon_window_impl` is no longer nested inside the `share_backend`
guard (`frontends/gpui/src/lib.rs:2423`), which now covers only
`ShareTrigger`. The bus is threaded from `main.rs` (hard `resolve`) into
both production launchers, and through `mobile.rs`. RED LOGS: registration —
`frontends/gpui/tests/degraded_signal_bus_container.rs` red on the
unmodified tree with `(ServiceNotProvided) - No provider registered for
type: alloc::sync::Arc<holon_loro::degraded_signal_bus::DegradedSignalBus>`
for `crdt.enabled = None` while the `Some(true)` case PASSED — the
mode-dependence in one run; subscriber —
`frontends/gpui/tests/degraded_bus_bridge_windowed.rs` (windowed, real
`TestApp`, SqlOnly container, production launcher) red under a revert-probe
that re-imposes the old `share_backend` gating: *"the production window
launch must subscribe to the DegradedSignalBus in SqlOnly mode"*,
`subscriber_count()` 0 after launch. Both green after. HONEST CAVEAT
(verifier's): the hard-error path itself has NO in-tree test — no container
in the repo now omits the bus, so the panic is unexercised; it is a
boot-time guard, not a covered behaviour. The windowed test asserts a
subscriber EXISTS and survives delivery, not that a banner is painted —
end-to-end paint remains covered only by `share_ui`'s existing
`apply_degraded` unit tests.
