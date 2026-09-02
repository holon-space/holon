---
id: 2026-09-02-desktop-sharing-unreachable-by-default-and-the-error-names-the-wrong-cause
date: 2026-09-02
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Desktop defaults `crdt.enabled` to false while mobile force-enables it, so a
  desktop-to-phone share is impossible out of the box, and the resulting error
  blames a missing entity provider without ever mentioning the config toggle.
---

## Bug

Found by the `double-dogfood` lane on 2026-09-02 while pairing the macOS desktop
app with the Android app.

A freshly launched desktop instance boots with `loro: false`. Every sharing
operation then fails:

```
Operation 'share_subtree' on 'tree' failed: … No provider registered for
entity: tree
```

The message is accurate about the symptom and silent about the cause. Nothing
in it, and nothing in the boot output beyond a single `loro: false` token on a
long configuration line, connects the failure to `crdt.enabled`. Read cold it
looks like a missing feature or a bad build, which is where it sent this lane
first.

Mobile does not have the problem, because `frontends/gpui/src/mobile.rs` passes
`true` for the CRDT argument of `load_runtime_with_platform_overrides` and
records why: without it `LoroModule` is never configured, so share and accept
fail. The desktop has no equivalent, and
`crates/holon-frontend/src/config.rs:559-560` defaults the flag to `false`:

```rust
pub fn crdt_enabled(&self) -> bool {
    self.crdt.enabled.unwrap_or(false)
}
```

So the exact configuration Martin wants — share from the laptop, accept on the
phone — cannot work until someone knows to set `HOLON_CRDT_ENABLED=true` or the
`[crdt]` section in `holon.toml`. Once set, sharing works immediately.

A second, smaller edge of the same problem: an operation name that does not
exist produces the identical text. `execute_operation` with a misremembered
`block`/`update` returns "No provider registered for entity: block" even though
`block` is registered and `create` on it had just succeeded. The tracing log is
better than the surfaced error — it adds `(operation: 'update')` and lists every
available entity — but the MCP reply drops both, so the two very different
failures are indistinguishable to the caller.

## Root cause

Two platforms disagree about a default that sharing depends on, and the
disagreement is only encoded on one side. Mobile force-enables CRDT at its boot
seam with a comment explaining the consequence; desktop inherits the global
`unwrap_or(false)`. Nothing reconciles them, and nothing at the point of failure
knows that the missing `tree` provider is downstream of a config flag.

## Missing piece

No test boots two instances with default configuration and tries to share
between them. Every sharing test constructs its backends directly, so the flag
is always on by construction and the default-off desktop path is never
exercised. The keystone PBT runs one instance and does not share at all.

There is also no fail-loud guard at the point where sharing becomes impossible.
`configure_mcp` in `frontends/gpui/src/di.rs:72-78` shows the pattern that is
missing: when MCP is disabled it logs one line saying so, explicitly because a
silent absence would violate fail-loud. CRDT has no such line.

## Remedy

FIXED (ruling D69.a, Martin 2026-09-02) — candidates 1 and 3 below, together:

- `HolonConfig::crdt_enabled` now defaults to `true`
  (`crates/holon-frontend/src/config.rs`). Every platform ships the CRDT layer
  on; SqlOnly is reached by an explicit `crdt.enabled = false`, and stays a
  first-class mode. Pinned by `desktop_default_enables_crdt` /
  `first_run_with_no_config_file_enables_crdt` /
  `explicit_false_still_disables_crdt` in the same file.
- A container that switches an entity off now says which setting did it.
  `add_frontend` logs the disclosure line at boot and registers
  `UnavailableEntities` for `tree` when the layer is off
  (`crates/holon-app/src/wiring.rs`); the dispatcher's not-found branch reports
  that reason instead of a bare missing registration
  (`crates/holon/src/api/operation_dispatcher.rs`). A share dispatched with
  `crdt.enabled = false` now fails with "Entity 'tree' is unavailable in this
  session: the CRDT layer is off (`crdt.enabled = false`); sharing needs it".
  Pinned by `crates/holon-app/tests/sharing_requires_crdt.rs`.
- The second edge is half-closed: every not-found refusal now carries the
  operation name, so `block`/`update` reads as an unknown operation rather than
  an unregistered entity. The available-entity set still reaches only the
  tracing log, not the MCP reply.

The three candidates as originally recorded:

1. Log the same one-line disclosure at boot that `configure_mcp` logs, naming
   `crdt.enabled` and stating that sharing is unavailable. Cheapest, and it
   removes the wrong-cause hunt.
2. Have the `tree` provider's absence report the reason, so
   `share_subtree` fails with the toggle's name rather than with a generic
   provider lookup miss. Parse-don't-validate would put this at the boundary:
   the operation is not merely unregistered, it is unavailable for a known
   reason, and that is a different state.
3. Default `crdt.enabled` to true on desktop as mobile already does. This is a
   product call, not a lane call, and it is entangled with whether sharing is
   ready to point at real vaults at all — see
   [2026-09-02-structural-edits-in-a-shared-subtree-never-reach-the-peer](2026-09-02-structural-edits-in-a-shared-subtree-never-reach-the-peer.md).

Separately, the MCP error surface should carry what the log already has: the
operation name and the available set. Losing it between the tracing layer and
the caller is the part that makes an unknown operation look like a broken
install.
