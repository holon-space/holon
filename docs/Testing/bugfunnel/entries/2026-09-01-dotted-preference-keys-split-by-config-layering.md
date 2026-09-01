---
id: 2026-09-01-dotted-preference-keys-split-by-config-layering
date: 2026-09-01
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Every preference stored in holon.toml was silently discarded on the desktop
  boot path, because the layered config loader split each dotted key into nested
  tables that no reader could match.
---

## Bug

A `shopping.list_url` written into `~/.config/holon/holon.toml` never reached
the running app: the Settings row painted `Not set` after a restart, and the
`${SHOPPING_LIST_URL}` a sidecar references stayed unresolved, so the connector
failed to build.

Found by an adversarial **verifier** driving a live app on port 8720 with a
throwaway profile (lane `c2-settings`, verification report
`c2-settings-verify.md`) — outside any automated test. The verifier isolated it
with a lane-independent control: `ui.theme` placed in the same `[preferences]`
map is dropped identically, and only *appears* to persist because
`set_preference` mirrors it into the typed `[ui]` field
(`crates/holon-frontend/src/config.rs:657-670`). Keys with no typed mirror —
`todoist.api_key`, `shopping.list_url` — were lost outright. Base-attributed:
the defect predates the lane.

## Root cause

Not a missing read leg. `HolonConfig.preferences` is a FLAT map whose keys
contain dots, but the layered pipeline addresses values by dotted **path**.
`load_config` (`crates/holon-frontend/src/config.rs:390-421`) layers
`Defaults → Toml::file → ClapSource` through premortem, which flattens
`preferences."shopping.list_url"` to the path `preferences.shopping.list_url`
and rebuilds it as nested tables. The map therefore deserialized as:

```
{PrefKey("shopping"): Table({"list_url": String("https://…")}),
 PrefKey("ui"):       Table({"theme":    String("dracula")})}
```

Measured, not inferred — this is the failure output of the red run,
`lane-logs/red-settings-diskleg-red.log`. The resolver
(`crates/holon-frontend/src/integration_vars.rs`) then filters each entry with
`v.as_str()?`; a `Table` yields `None`, so every preference dropped out and
`mcp_integrations.rs` built its `${VAR}` lookup from an effectively empty map.

The two notions of `.` collide: a flat map of dotted keys cannot round-trip
through a dotted-path flattener.

## Missing piece

No test ever loaded a preference from a real file. Every rung — the lane's own
included — constructed `HashMap<PrefKey, toml::Value>` in memory and handed it
straight to the resolver, which is precisely the step that the boot path gets
wrong. The keystone PBT does not boot through `load_config` at all, so the
prod-only config-layering wiring is invisible to it: the interaction (store a
preference, restart) is generatable in principle, but the failing code path does
not exist in the test environment. Hence ENVIRONMENT, with COVERAGE secondary
for the absent file-round-trip rung.

## Remedy

Fixed generically for **all** preferences — no per-key mirror:

- `PrefKey::parse` (`crates/holon-frontend/src/preferences.rs`) — the fallible
  form, so config input cannot panic the boot. `PrefKey::new` delegates to it,
  and `Deserialize for PrefKey` now reports a serde error instead of panicking.
- `deserialize_preferences` (same file) collapses nested tables back into dotted
  keys, recursing to the scalar. A preference value is always a scalar, so a
  table can only ever be a split key — the collapse is unambiguous and accepts
  both the flat shape (a direct parse, i.e. the mobile `load_runtime` path) and
  the nested shape (the layered desktop path).
- Wired via `deserialize_with` on `HolonConfig.preferences`
  (`crates/holon-frontend/src/config.rs`).

Pinned by two rungs that were **red before the fix**:

- `config::tests::load_config_applies_persisted_preferences` — the desktop
  loader must return the authored dotted key. Red log:
  `lane-logs/red-settings-diskleg-red.log`.
- `a_list_url_persisted_in_holon_toml_configures_the_connector_after_a_restart`
  (`crates/holon-app/tests/settings_shopping_list_url_credential.rs`) — a real
  `holon.toml` on disk must configure the REST connector and register the token
  with the redactor.

Keystone repro: not reproducible there, and not made so. The composed keystone
never boots through `load_config`; closing that parity would mean giving it a
real config-dir boot rung, which is prod/test-parity work beyond this fix and is
the ENVIRONMENT remedy this entry records.
