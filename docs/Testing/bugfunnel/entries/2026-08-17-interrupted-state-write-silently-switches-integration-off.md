---
id: 2026-08-17-interrupted-state-write-silently-switches-integration-off
date: 2026-08-17
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An interrupted write of an integration's state file left it zero-byte, and
  the loader read a zero-byte file as "never touched", silently switching a
  configured, enabled integration off at the next boot.
---

## Bug

Found by an adversarial `verifier` subagent probing the new tri-state
integration config store (lane `lane-integrations`, task D2.a). Evidence in
`lane-integrations-verify.md` §4d; the verifier's own probe logs are
`/tmp/verify-integrations-probe.log` and `/tmp/verify-integrations-probe2.log`.

The lane claimed, in code, test doc, and lane report, that "a state file that
exists but does not parse is fatal". The verifier drove
`IntegrationConfigStore::load` over a table of degraded state files and found
six of them loading `Ok` with defaults substituted: a zero-byte file, a
newline-only file, a file truncated after its first line, a typo'd key
(`enabledd`), a renamed key (`enable`), and a file with an unknown extra key.
Only the single type-mismatch input the lane's own test used actually failed
loud.

The user-visible consequence: an integration the user had enabled and
configured comes back at the next boot disabled and unconfigured, with no
error anywhere. That is priority-4 behaviour under the repo's error policy —
silently degrading to look fine.

## Root cause

Two mechanisms compounding, both in `crates/holon-mcp-client/src/integration_state.rs`:

1. **The write was not atomic.** `set` used `std::fs::write`, which opens with
   `create(true).truncate(true)`. Between the truncate and the write the file
   is exactly zero bytes. A crash, power loss, or ENOSPC in that window leaves
   a zero-byte state file. ENOSPC is not hypothetical here — open task #13
   records ~1.7TB of lane target directories filling the volume and killing
   builds.

2. **The read treated an incomplete file as a complete one.** `IntegrationState`
   derived `Deserialize` with `#[serde(default)]` on both fields and no
   `deny_unknown_fields` and no schema version, so an empty document parsed
   happily into the all-defaults value — which is exactly the "never touched,
   off, unconfigured" state. The same laxity meant a renamed or typo'd key was
   dropped and defaulted rather than rejected, so any future schema change
   would silently reset every user's state. That is asymmetric with the
   sidecars these files sit beside, which carry `schema_version: 1` and have an
   explicit drift path (`crates/holon-mcp-client/tests/sidecar_drift.rs`).

## Missing piece

**No generation over degraded state files.** The oracle existed — a test named
`a_corrupt_state_file_fails_loud_with_provider_and_path`, asserting exactly the
right property — but it was fed a single input, `enabled = "yes, very"`, which
is the one class serde rejects without any help from the schema. Every silent
degradation left it green. A property asserted over one point is not a
property.

Secondary, and the reason this is dual: **there is no fault-injection rung for
interrupted writes anywhere in the harness.** No test environment can produce a
crash between truncate and write, so the zero-byte file could only be reached
by reasoning about the write path, never by running it. This is the same
missing capability as open task #20 (corrupt-marks fault-injection rung).

The keystone PBT (`general_e2e_composed_pbt.rs`) cannot reproduce this: it has
no integration-state transitions at all, since the store drives no consumer
yet. Closing that is part of the store's cutover, not of this fix.

## Remedy

Fixed in the same lane, red-first. The corruption test became
`every_corrupt_state_file_fails_loud_with_provider_and_path`, a probe table of
nine degraded inputs built by mutating the canonical file the store itself
writes, accumulating every escape so the failure names the whole table. Red log
`/tmp/lane-integrations-r2-RED.log` — 8 of 9 inputs degraded silently. Then:

- `set` writes a `.toml.tmp` sibling, `sync_all`s it, and renames it over the
  target, so the file is never observable half-written and the temporary stays
  out of the `*.yaml` sidecar scan. Pinned by
  `a_write_leaves_no_partial_file_behind`.
- The on-disk form is a separate `StateFile` type with
  `deny_unknown_fields`, every field required, and a required
  `schema_version` checked against `INTEGRATION_STATE_SCHEMA_VERSION` — so an
  empty, truncated, hand-edited, or older-generation file is a parse error
  naming provider and path, and only a MISSING file means "never touched".
  `IntegrationState` stays the permissive in-memory domain type.

Green log `/tmp/lane-integrations-r2-GREEN.log`, 8/8.
