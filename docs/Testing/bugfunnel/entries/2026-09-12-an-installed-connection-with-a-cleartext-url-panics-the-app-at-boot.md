---
id: 2026-09-12-an-installed-connection-with-a-cleartext-url-panics-the-app-at-boot
date: 2026-09-12
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A user-dropped connection file whose tool URL is cleartext http to a
  non-loopback host panics holon-gpui during boot, so the app never opens a
  window and the file cannot be switched off from inside the app.
---

## Bug

Found by the `dogfood-explorer` gate for the landed `user-connections` feature
(main `f134df9ece6c`), driving the real GPUI app.

A synthetic connection file `cleartexthost.yaml` was written into the sandbox
integrations directory. It is a copy of the shape the bundled
`assets/integrations/jsonplaceholder.yaml` demonstrates, with the tool URL
changed to `http://api.fixture-notreal.example/items` and its secret reference
renamed into its own namespace. It was switched on with
`scripts/holon-integration-enable.sh`, which is the documented remedy the app's
own disclosure prints.

With `CLEARTEXTHOST_TOKEN` unset, the connection is skipped politely: a red
toast reads `Integration unavailable — cleartexthost: Variable
'CLEARTEXTHOST_TOKEN' referenced in config is not set`. The scheme rule is never
reached, because the variable is resolved first.

With the variable set, the app **dies during boot**. Nothing listens on the MCP
port, no window appears, and the only trace is one log line:

```
ERROR ... holon_frontend::logging: PANIC: [McpIntegrationsModule] Invalid
integration config for provider 'cleartexthost': the `url` of tool 'list-items'
uses http://, but a connection must use https — a cleartext endpoint sends this
connection's credentials in the clear and lets anyone on the path choose what
lands in the vault. (http:// to a loopback address is the only exception.)
```

Evidence: `scratchpad/dogfood-uc/logs/app5.log` (the panic at line 192; the log
ends mid-boot), and `lsof -tiTCP:8710 -sTCP:LISTEN` empty afterwards. The same
tree boots fine once the file is removed (`logs/app6.log`).

The refusal TEXT is exactly right — it names the tool, names the scheme, states
the reason, and does not quote the URL. The delivery is the defect.

## Root cause

`crates/holon-app/src/mcp_integrations.rs:627-632`. Building the runtime config
for each enabled provider matches on the error: an `UnresolvedVar` is a disclosed
skip that `continue`s, and **every other error panics**. The comment beside it
states the trade deliberately: "the DI factory has no Result channel, so panic
with full context rather than silently skipping."

That trade was sound while every sidecar was compiled into the binary and
therefore reviewed. The `user-connections` feature made the roster a union of
the bundle with whatever regular `.yaml` files a user drops into
`{config_dir}/integrations/`, so this panic is now reachable from untrusted
content. One malformed file — the exact case the scheme rule exists to catch —
denies the user their whole application, including the Settings toggle that is
the documented way to switch the offending connection off.

This also contradicts the project's own D94.a ruling, which took the analogous
unsatisfiable-re-import case from an `.expect` stop to a DEGRADED boot with a
sticky banner and a retry action.

## Missing piece

ENVIRONMENT. `parse_secure_call_url` is well covered at the unit level: the lane
report's Inc 1 red log shows four cases asserting the refusal, and they are
green. None of them runs through the DI boot path that panics, so every test
sees the `Err`, and no test sees what the app does with it. The refusal is
proven; its delivery to a user is not tested anywhere.

Secondary COVERAGE: no test — headless keystone or windowed — boots the app with
an ENABLED, INSTALLED connection whose config is invalid for a reason other than
an unresolved variable. The seven windowed integration tests under
`frontends/gpui/tests/` all use bundled providers.

The keystone PBT cannot reproduce this: it has no transition that writes a file
into the integrations directory and no boot-with-installed-sidecar arm. Parity
work needed: a boot rung that assembles `McpIntegrationsModule` over a
caller-supplied integrations directory and asserts the app reaches a usable
state with a disclosure, for each `IgnoredReason`/invalid-config arm.

## Remedy

FIXED in lane `uc-fixes` (wave 12).

`crates/holon-app/src/mcp_integrations.rs` — the invalid-config arm of the
connect loop now does what the `UnresolvedVar` arm does: log, disclose, record
`Unavailable`, `continue`. The disclosure is a new helper
`disclose_unusable_config`, raising the SAME sticky condition the directory
scan's `Unusable` arm raises (`IntegrationSidecarUnusable`), because from the
user's side both are "this file names a connection that cannot be used, edit
it". It carries the file path, taken from the settings row's `origin` — the
connect loop already read that row for its display name.

Covered by `crates/holon-app/tests/invalid_connection_config_boots_degraded.rs`,
a real boot through `new_from_config_with_di` over a config directory holding
the offending file. Red log: `lane-logs/item1-RED-1789178108.log`, failing with
the panic itself at `mcp_integrations.rs:628`. Green:
`lane-logs/item1-GREEN-1789178527.log`.

## Attribution

REGRESSION of `user-connections` in reach, not in code. The panic predates the
lane (`git show a5e161c0:crates/holon-app/src/mcp_integrations.rs` has the same
arm), and while every sidecar was compiled in it was a correct programmer-error
stop. The lane made those files user-supplied, so the same line became "one bad
file costs the user their application". The second test in the file records the
other half of the A/B: the scan-seam refusals (foreign secret namespace, and by
the same path inline secret / schema / symlink) were ALREADY disclosed and never
panicked — it passes before the fix as well as after.

## The remedy this fix restores DOES work — an earlier note here was wrong

The argument for a degraded boot is that the user keeps the Settings switch that
turns the offending connection off, so whether that switch is reachable is part
of whether this entry is really fixed.

An earlier revision of this section said it was not: a measurement on the
windowed surface showed rows 2 to 6 of Settings › Integrations reporting 3 px
and then 0 px of height, with a scroll-wheel event changing nothing. **That
reading was wrong and is withdrawn.** Lane `sidecar-sync-fixes` refuted it:

- The zero heights are window-mask CLIPPING, not a layout collapse — the panel
  carries `max_h(720px)` inside a 900 px window, so rows below the mask report
  no painted box.
- The wheel appeared inert only because gpui drops a scroll event that is not
  preceded by a `MouseMoveEvent`. With a pointer move first, all six rows come
  into view, and a real click on the LAST row flips that provider and only that
  provider.

Pinned by `frontends/gpui/tests/settings_integrations_last_row_toggle_windowed.rs`;
the finding itself is recorded as FALSE-ALARM in
`2026-09-12-only-the-first-settings-integrations-row-can-be-clicked.md`.

So the remedy this entry depends on works: a user whose refused connection sorts
below the fold scrolls to it and switches it off. This entry is FIXED without a
dependency on another lane.

Two real UX gaps remain, neither of which blocks the remedy and both of which
are follow-ups rather than entries: the list offers no scrollbar affordance, so
nothing on screen says there is more below; and the 720 px cap sits inside an
868 px viewport, wasting height that would have shown the rows without any
scrolling at all.

My own latency rung still drives the FIRST row, and that is now a property of
the rung rather than of the product — it performs no pointer move and no scroll,
so it can only reach what the initial mask shows.
