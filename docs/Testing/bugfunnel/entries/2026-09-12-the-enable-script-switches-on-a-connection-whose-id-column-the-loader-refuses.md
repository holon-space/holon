---
id: 2026-09-12-the-enable-script-switches-on-a-connection-whose-id-column-the-loader-refuses
date: 2026-09-12
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  holon-integration-enable.sh reports "Enabled 'intid'" and writes the state
  file for a connection whose INTEGER identity column the app then refuses, so
  the script and the app still disagree about which connections exist.
---

## Bug

Found by the `user-connections` dogfood RE-RUN of 2026-09-12, scenario 4
(script and toggle must agree), at `main` `e50f042ba5b3`.

A directory holding three files — a valid connection, a symlinked one, and one
declaring its identity column `INTEGER` — was offered to the enable script:

```
$ HOLON_MCP_INTEGRATIONS_DIR=$D scripts/holon-integration-enable.sh linkedthing
error: nothing provides an integration 'linkedthing'.          exit 1   correct
$ HOLON_MCP_INTEGRATIONS_DIR=$D scripts/holon-integration-enable.sh intid
Enabled 'intid' — wrote .../intid.state.toml                   exit 0   wrong
```

The app, launched over that same directory, refuses `intid`:

```
'…/intid.yaml' names connection 'intid' but cannot be used, so that connection
does not exist: sidecar entity 'intid_items' declares its identity column `id`
as `INTEGER`. … declare it `sql_type: TEXT`.
```

So the user is told the connection is on, and the application says it does not
exist. This is the same shape as the symlink disagreement recorded in
`2026-09-12-the-enable-script-switches-on-a-symlinked-connection-the-loader-refuses`,
reached through a different rule; the symlink half of that entry is confirmed
fixed by this same run.

Evidence under `/tmp/holon-dogfood-uc/`:

- `logs/conn-list.log` — `holon-connection list` naming `intid` as admitted and
  `linkedthing` as ignored.
- `s5-app-table.log` — the app refusing `intid` from the directory the script
  had just written.
- `s5/shots/16-table-bottom.png` — the resulting Settings row: `intid` switched
  on, and a toast saying the file cannot be used.

## Root cause

`scripts/holon-integration-enable.sh` asks the binary
`crates/holon-mcp-client/src/bin/holon_connection.rs`, which reports what
`ConnectionRoster::scan` admits. That was the right correction — it removed the
script's private copy of the rules — but the roster is not the whole admission
decision.

The app refuses `intid` one seam later, in `MirrorSchema::parse`
(`crates/holon-mcp-client/src/mcp_sidecar.rs`), which the roster scan does not
run. Any rule that lives at sidecar-load rather than at roster-scan is
therefore invisible to the script, and the identity-column rule is the first
such rule this feature shipped.

The script's own comment states the intent exactly: "This script does not decide
for itself which connections exist — the loader does, and that is what keeps the
two from disagreeing." The lookup it performs reaches only half of the loader.

## Missing piece

`crates/holon-mcp-client/tests/…` states the script-vs-loader property as
agreement, but only over the roster's verdict. No case offers the script a file
that the ROSTER admits and the SIDECAR LOAD refuses, so the half of the
admission decision the script cannot see is also the half no test compares.

The property worth pinning is the whole one: for every file in a directory, the
script enables it if and only if a booted app would run it.

## Remedy

FIXED by lane `uc-fixes-2`.

**The pin, red first.** `crates/holon-mcp-client/tests/enablement_cutover.rs`
→ `the_enable_script_refuses_a_connection_whose_id_column_the_loader_refuses`.
Stated as AGREEMENT, like its symlink neighbour: the loader's verdict is
measured first (switched on by hand, then `load_integration_configs` refuses it
as `Unusable`), and only then is the script offered the same directory. Red for
the right reason: `Enabled 'intid' — wrote …/intid.state.toml`.

**The fix.** `ConnectionRoster::scan_loadable` (`crates/holon-mcp-client/src/roster.rs`)
is `scan` minus every introduced connection whose file the loader would then
refuse for its CONTENT. It runs the same seam the app runs —
`integration_config::content_verdict`, which wraps `choose_content_for` and
therefore `McpSidecar::from_yaml` — plus `check_secret_namespace`, the other
rule that lives past the scan. `holon-connection list` asks it instead of `scan`.

Separate from `scan` rather than folded into it: the load path needs the refused
entry to stay in the roster so its Settings row and its refusal toast still name
the file. `scan_loadable` is for the callers that need the verdict WITHOUT
booting. The script's stderr now carries the loader's own words, so the reason
the user reads is the reason the app gives.

## Attribution

Regression of lane `uc-fixes`, which corrected the script to ask the binary
rather than parse the source. That was the right correction and it closed the
symlink disagreement; it reached only the roster's half of the admission
decision, and the identity-column rule was the first rule this feature shipped
on the other side of that seam.
