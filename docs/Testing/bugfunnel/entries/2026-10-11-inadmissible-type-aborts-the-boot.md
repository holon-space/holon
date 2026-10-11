---
id: 2026-10-11-inadmissible-type-aborts-the-boot
date: 2026-10-11
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A user-installed integration sidecar can declare a datatype whose home
  cannot carry it. It either aborted the whole boot ("refusing to start") or
  was never admitted; the integration must be refused at load and the boot
  must continue.
---

## Bug
Code audit against the boot-always directive (2026-10-08) and ruling
AUDIT-boot.a, lane type-admission-boot. The CV-E admission sweep ran after the
engine was built (`crates/holon-app/src/session.rs:90-104` on main 821d6c0778aa)
and turned any refusal into `Err("refusing to start: ...")`.

For compiled-in types (bundled registry, kitchen, `File`) that abort is right:
a refusal is a wiring fault. But the fault is also reachable from data. A
sidecar YAML in the integrations directory declares its mirror schema as
`FieldSchema`s, which deserialize a per-field `home:` and a `lifetime`
(`crates/holon-api/src/entity.rs:424-460`,
`crates/holon-mcp-client/src/mcp_sidecar.rs:81`). The integration registers
those types in `go_live` (`crates/holon-app/src/mcp_integrations.rs`), after
`build_mcp_integration` has already created the entity cache table, the
`integration_cache` row, the FDW tables and the sidecar views. Depending on
timing, a bad `home:` either aborted the boot or was never admitted at all.

## Root cause
Sidecar types had no admission at their own door. The only check was the
registry sweep, which treats every refusal as a wiring fault, and which runs
at a time unrelated to when an integration registers its types.

## Missing piece
No test drove a sidecar with an inadmissible `home:` through the real load
path. The keystone authors no sidecar with a `home:` field, so it cannot
generate the state. The ENVIRONMENT part is the go-live-versus-sweep timing,
which differs between `wait_for_ready` frontends and the others.

## Remedy
- Rulings (steward5, 2026-10-12): a bad sidecar type refuses the WHOLE
  integration as one unit; a refused bundled type stays a boot abort (wiring
  fault); sidecar types are admitted at sidecar load, before
  `build_mcp_integration`.
- `McpIntegrationsModule::from_dir` parses each enabled sidecar
  (`IntegrationFileConfig::sidecar`) and admits its types
  (`type_admission::sidecar_refusals`). A refused integration leaves the
  config list, so nothing connects, builds or registers for it. The supervisor
  raises `ConditionKind::IntegrationTypeRefused` per refused type (integration,
  field, home, reason; subject = the type) and records the integration
  `Unavailable`.
- The registry sweep at session construction is unchanged from main: it aborts.
- Pinned by
  `sidecar_type_admission::a_sidecar_entity_its_home_cannot_carry_refuses_the_integration_and_builds_nothing`
  (red on the go-live admission: the cache table `gadget_items` existed after
  the refusal; green after) and
  `type_admission_cve::a_sidecar_field_naming_an_unknown_home_is_refused_by_name`.
