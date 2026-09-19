---
id: 2026-09-19-an-unusable-connection-file-leaves-its-row-on-pending
date: 2026-09-19
gap: ORACLE
status: OPEN
summary: >-
  `github` is enabled by a state file but this build ships no such sidecar, so
  no connect is ever attempted and its `integration_state` row sits on `Pending`
  forever while the sidebar paints it as a half-finished connection.
---

## Bug

Found by the `dogfood-integ` lane against a copy of Martin's real config dir
(a throwaway copy of the real config dir), MCP port 8720.

`integrations/github.state.toml` says `enabled = true`. This build bundles no
`github` sidecar, and the installed `github.yaml` is refused at load — loudly
and correctly, with a toast that names the file and the exact parse failure.

The mirror row, however, reads:

```
github  enabled=1  status=Pending  display_name=Github  default_view=null
```

and the sidebar paints it between Google Calendar and Gmail with a half-filled
status glyph — visually "connecting", indefinitely. Clicking it can only refuse,
because `default_view` is null.

Gmail and Shopping, which are bundled but unconfigured, correctly reach
`Unavailable` (hollow red glyph). Github is the only row that is neither
connected nor disclosed in the list itself.

Secondary, cosmetic: `display_name` is derived as `Github`, not `GitHub`.

## Root cause

`Pending` is the initial value of a mirror row, overwritten by whatever the
connect registry decides. For a provider the build does not ship, the connect
loop never runs, so nothing overwrites it. The boot WARN and the toast disclose
the FILE; nothing reconciles the ROW.

This is the same shape as
`2026-08-18-integrations-section-shows-one-stale-row` — a status write that
never happens leaving `Pending` behind — but reached by a different route, so
the rung that closed that one
(`crates/holon-integration-tests/tests/frontend_suite/integration_state_boot_records_status.rs`)
does not see it: that test enables only providers the build ships.

## Missing piece

No rung enables a provider name the build does NOT bundle and then asserts the
mirror. The `assets/integrations/README.md` contract says such a file "is
disclosed at boot with a WARN and a toast"; it does not say what the row must
read, and so nothing checks it.

## Remedy

Open. An enabled provider with no bundled sidecar should be projected as
`Unavailable` with the load failure as its cause, not left on `Pending` — or it
should not be mirrored into the discovery list at all, since it can never become
a connection. Extend the boot rung to enable an unshipped name and assert the
resulting status.
