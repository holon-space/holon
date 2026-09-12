---
id: 2026-09-12-the-enable-script-switches-on-a-symlinked-connection-the-loader-refuses
date: 2026-09-12
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  holon-integration-enable.sh reports "Enabled 'linkedthing'" for a symlinked
  sidecar the loader refuses, so the script and the loader disagree about what
  exists and the user is told the opposite of what happens.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`).

`linkedthing.yaml` was created in the sandbox integrations directory as a
symbolic link to a valid sidecar stored outside that directory. Increment 5's
addendum refuses exactly this, correctly and with a good message.

The enable script does not:

```
$ HOLON_MCP_INTEGRATIONS_DIR=... scripts/holon-integration-enable.sh linkedthing
Enabled 'linkedthing' — wrote .../linkedthing.state.toml
Restart Holon to pick it up.
```

Restarting produces two disclosures instead, and the connection does not exist:

```
WARN ... 'linkedthing.yaml' names connection 'linkedthing' but cannot be used ...
     it is a symbolic link, and a sidecar is read only from a regular file ...
WARN ... 'linkedthing.state.toml' refers to connection 'linkedthing', which
     nothing provides ... Delete the leftover file, or add a 'linkedthing' ...
```

Evidence: `scratchpad/dogfood-uc/logs/app4.log`, screenshot
`shots/05-refusals.png`. The script also leaves a `.state.toml` behind that the
loader then has to complain about separately, so one wrong "Enabled" produces
two warnings the user must now clean up.

## Root cause

The script's existence check was widened in Increment 5 to accept an installed
sidecar: it parses `bundled_sidecars.rs` and additionally looks for
`<provider>.yaml` in the target directory. That look-up tests for the file's
presence, not for the same admissibility the loader applies. The symlink refusal
landed later in the same increment, in `scan_installed_sidecars`, and the script
was not revisited.

So the two now answer different questions. The loader asks "is there a usable
regular file introducing this name"; the script asks "is there something at this
path".

## Missing piece

COVERAGE. The lane added a case asserting "the script and the loader agree on
what exists", and it is green — because it was written before the symlink
refusal existed and exercises a regular file. The agreement property is the
right property; its instantiation covers one of the two ways a file can be
inadmissible.

The general shape the test should take: for each way a file is refused
(symlink, non-regular file, unparseable, wrong `schema_version`, foreign secret
namespace), the script and the loader must give the same answer. Today only the
"file absent" and "regular usable file" ends of that range are covered.

The keystone PBT cannot reproduce this; the enable script is not in its wiring.

## Remedy

FIXED in lane `uc-fixes` (wave 12), by the first of the two options above — the
one that removes the class.

- `crates/holon-mcp-client/src/bin/holon_connection.rs` (new binary
  `holon-connection`) — `holon-connection list <DIR>` prints what
  `ConnectionRoster::scan` admits, one name per line, and reports each refused
  file with the loader's own reason on stderr.
- `scripts/holon-integration-enable.sh` — no longer parses `bundled_sidecars.rs`
  and no longer globs `*.yaml`. It asks the binary, resolved through
  `$HOLON_CONNECTION_BIN`, then a built `target/{release,debug}` copy, then
  `cargo run` from the checkout. That chain is a lookup, not a rule, so it
  cannot drift. The loader's reason passes through to the user, because the
  remedy for a symlink is to replace it with the file.

Covered by `the_enable_script_refuses_what_the_loader_refuses` in
`crates/holon-mcp-client/tests/enablement_cutover.rs`, stated as AGREEMENT
(it asserts the loader's verdict first, then that the script matches) so the
rule stays in one place. The shared `run_enable` helper now passes
`CARGO_BIN_EXE_holon-connection`, so every script test runs against this build's
binary.

Red: `lane-logs/item4-RED-1789179688.log`, produced by putting the base-rev
script back in place (`git show f134df9ece6c:scripts/…`) — it printed
`Enabled 'linked-thing'`. Restored byte-for-byte, sha256 `f078bb7a…` identical
before and after (`lane-logs/item4-teeth-before.txt`, `item4-teeth-after.txt`).
Green: `lane-logs/item4-1789179561.log`, 15 of 15.

## Attribution

REGRESSION of `user-connections`. Increment 5 taught the script to accept
introduced connections by globbing the directory, and the symlink refusal is the
same lane's. Neither existed at `a5e161c0`, where the script knew only bundled
names.
