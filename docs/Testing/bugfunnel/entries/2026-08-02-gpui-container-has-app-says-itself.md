---
id: 2026-08-02-gpui-container-has-app-says-itself
date: 2026-08-02
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The GPUI container has NO `DegradedSignalBus`, and the app says so itself at
  every boot: `[McpIntegrationsModule] No DegradedSignalBus in this container
  (ServiceNotProvided ... holon_loro::degraded_signal_bus::DegradedSignalBus)
  — integration connect failures will be LOG-ONLY and their pages will render
  blank with no banner` (`crates/holon-app/src/mcp_integrations.rs`, logged at
  ERROR). So the ONE disclosure channel designed for this failure mode is
  unwired in the shipping desktop frontend: any integration that fails to
  connect produces an empty page and no banner.
source_line: 1144
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710)
The GPUI container has NO `DegradedSignalBus`, and the app says so itself at
every boot: `[McpIntegrationsModule] No DegradedSignalBus in this container
(ServiceNotProvided ... holon_loro::degraded_signal_bus::DegradedSignalBus)
— integration connect failures will be LOG-ONLY and their pages will render
blank with no banner` (`crates/holon-app/src/mcp_integrations.rs`, logged at
ERROR). So the ONE disclosure channel designed for this failure mode is
unwired in the shipping desktop frontend: any integration that fails to
connect produces an empty page and no banner.

## Root cause

dogfood, boot log — `FrontendSession.resolve_engine` finds `No
DegradedSignalBus in this container`, and the module says so verbatim:
"integration connect failures will be LOG-ONLY and their pages will render
blank with no banner"; the degraded-mode disclosure channel the error
philosophy depends on is simply not wired in the shipped GPUI container)

## Missing piece

No test boots the GPUI DI container and asserts that every seam the modules
disclose through is actually provided. Missing piece = a
container-completeness test (resolve each disclosure seam after wiring) —
the app already knows the answer at runtime and only WARNs about it.

## Remedy

FIXED 2026-08-04 — same defect as the 2026-08-04 row above (first observed
here on 2026-08-02); see that row for the two-part fix (unconditional
registration in the composition root + unconditional disclosure bridge) and
its red logs.
