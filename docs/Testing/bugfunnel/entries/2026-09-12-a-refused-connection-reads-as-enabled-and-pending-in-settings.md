---
id: 2026-09-12-a-refused-connection-reads-as-enabled-and-pending-in-settings
date: 2026-09-12
gap: ORACLE
secondary: PERCEPTION
status: OPEN
summary: >-
  A connection file the loader refused still gets a Settings row reading
  Enabled ON, Status Pending, with no trace of the refusal — and the refusal
  toast's own overflow line sends the user to exactly that row for the reason.
---

## Bug

Found by the `user-connections` dogfood RE-CHECK of 2026-09-12, driving the
real GPUI app at `main` `060022da56b6`.

A single inadmissible connection file (`intid.yaml`, whose entity declares its
identity column `INTEGER`) was installed and switched on. The loader refuses it
correctly and says so in the log and — when the window is tall enough — in a
toast whose reason, path and remedy are all painted.

Settings › Integrations then shows the refused connection as an ordinary one:

| Column | Painted |
|---|---|
| Integration | `intid`, `from …ns/intid.yaml`, `calls` (no value) |
| Config | `unconfigured` |
| Status | `Pending` |
| Enabled | switch ON |

Nothing on that row says the file was refused, that the connection does not
exist, or that it will never run. `Pending` + a lit switch is exactly what a
connection that is about to come up looks like. This is the "silently degrades
to look fine" tier the project's error philosophy forbids.

The failure is compounded at short window heights. When the toast stack does
not fit, the overflow line reads *"and 1 more not shown — open Settings ›
Integrations for the full list"*. Following that instruction leads to the row
above, which carries no reason at all — so the one route the product offers to
the refusal text is a dead end, and the reason survives only in the log.

Evidence (`shots/` under the run directory copied to
`scratchpad/dogfood-uc-recheck/`):

- `G-01-short.png` — 1400x420, the overflow line pointing at Settings.
- `G-02-settings-short.png` — the `intid` row it points at: Pending, switch ON.
- `D-01-refusal.png` — 1400x900, the same file, toast fully painted.
- `runs/D/logs/app.log:12` — the loader's own WARN with the whole reason.
- `runs/G/logs/app.log` — `[IntegrationsSettingsVm] 'intid' sidecar did not read
  … its row falls back to the derived name and the default icon`.

## Root cause

The mirror has no place for the verdict. `SELECT * FROM integration_state WHERE
provider_name='intid'` returns `status=Pending`, `enabled=1`, and no column
naming a refusal; `crates/holon-app/src/integrations_section.rs:45` selects
`config_status, configurable, configure_progress, origin, hosts` and a `status`
whose only "unhealthy" vocabulary is the runtime sync states.

`holon-app/src/integrations_settings.rs` already KNOWS the file did not read —
it logs it once per session and falls back to a derived display name — but the
knowledge stops at the log line. The row it then writes is indistinguishable
from a bundled connection that has simply not connected yet.

## Missing piece

No assertion anywhere says what a REFUSED provider's Settings row must look
like. `refusal_toasts_reach_the_user_windowed.rs` pins the toast and stops
there; `settings_integrations_table_fits_windowed.rs` and
`settings_introduced_row_fits_windowed.rs` pin geometry over rows that were all
admitted. Between the two, the case "the loader said no and the table still
says Pending" is unowned.

The interaction is generatable — the windowed rungs already install refused
files — so this is an ORACLE gap, not a coverage one.

## Remedy

Open. Two halves, and the first is the real one:

1. Carry the verdict into `integration_state` (a refusal reason column, or a
   `status` value that is not one of the healthy ones) so the Settings row can
   state it, and paint it as the row's status with the reason reachable.
2. A windowed pin asserting that a provider whose file the loader refused
   paints neither `Pending` nor a lit Enabled switch without a refusal beside
   it — red for the right reason against today's build.

Until then the toast overflow line should not promise Settings carries the full
list, because it does not.
