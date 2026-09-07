---
id: 2026-09-08-sidecar-mirrored-type-has-no-stamp-home-panics-boot
date: 2026-09-08
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Booting with a connected sidecar panics the whole app whenever the connector
  mirrors an entity it does not write, because a sidecar-derived type never
  declares the overflow column the engine's `_provenance` stamp lands in. The
  same type is also built from the columns a remote MCP server advertises, so a
  server naming `properties` itself crashes the client it is connected to.
---

## Bug

Found by a verifier measuring an unrelated lane against the integration chain
tip `5c2db13e`: `boot_suite::mcp_mirrored_entity_write_authority::
mirrored_entity_keeps_the_connector_as_its_only_write_authority` failed 3/3,
deterministically, at `crates/holon/src/api/operation_dispatcher.rs:1655`.

    [OperationModule] write authority for free-standing type 'fk_fake_shadow':
    type 'fk_fake_shadow' declares no `properties` overflow column, so the
    engine's `_provenance` stamp would have nowhere to land and EVERY `create`
    and `update` of it would be refused at the write boundary. […] Declared
    persisted fields: ["id", "data"]

The failure is a `panic!` inside `OperationModule::configure`, so this is not a
test-only symptom: any vault whose sidecar mirrors an entity the connector does
not write crashes at boot, before any UI exists to disclose it.

## Root cause

`3f6e985e` ("two GENERIC synced-peer type rules") added
`require_engine_stamp_has_a_home` to `register_write_authority`
(`crates/holon/src/core/type_declaration.rs:109`) and swept the type-construction
sites it knew about onto the new `FieldSchema::overflow_pair()` — the hand-written
file DDL, the keystone datatype axis, the admission tests. It missed one:
`EntityConfig::to_type_definition` (`crates/holon-mcp-client/src/mcp_sidecar.rs`),
the site that turns a sidecar's `schema:` into the type every mirrored entity is
registered under by `register_sidecar_entity_types`.

A sidecar's `schema:` names the columns the connector mirrors, which is a
different set from the columns a row needs to be writable, so the derived type
carried no `properties` column. The boot loop derives a SQL write authority for
every free-standing type and skips only the ones a connector already claims — so
a mirrored entity the connector does NOT write reaches
`register_write_authority`, fails the new check, and panics.

Measured bisect (one workspace, `jj new` per probe, single positional test
filter, 4/4 deterministic):

| commit | what it is | result | log |
|---|---|---|---|
| `3a070e88` | parent of `3f6e985e` | PASS | `lane-logs/p2-3f6e985e-parent.log` |
| `3f6e985e` | sync-peer-types | FAIL | `lane-logs/p3-3f6e985e-types.log` |
| `a47d8de2` | pair-reimport (chain) | FAIL | `lane-logs/p1-a47d8de2-pair-reimport.log` |
| `5c2db13e` | chain tip | FAIL | `lane-logs/p4-5c2db13e-tip-RED.log` |

The adjacent pass/fail pair localises the regression to `3f6e985e`, five commits
BELOW the chain base — the chain inherited it rather than introducing it.

## Second producer: the remote server

An entity's columns do not only come from the sidecar file. `finish_integration`
overwrites them from the connected server's resource templates — a merge into a
sidecar entity that declared no `schema:`, or an insert of an entity the file
never mentioned — and `parse_resource_template_meta` builds every advertised
field as an ordinary declared column.

A server that advertises a field named `properties` or `property_kinds`
therefore reaches the type registry with the exact shape the panic refuses,
without touching any file on the machine. The trigger is remote: a connected
sidecar server, which is untrusted input, crashes its client at boot. The load
-time refusal on the YAML leg does not stand on this path, because the schema is
replaced after the file was parsed.

## Missing piece

The covering test already existed, had the right oracle, and goes red for the
right reason. It was simply never executed: `3f6e985e`'s gate crate set did not
include `holon-integration-tests`, the consumer crate that holds `boot_suite`,
so the guarantee the commit added was never measured against the one harness
that boots a connector.

Nothing else could have caught it. The composed keystone PBT implements no
`SutAppLifecycle`, so `StartApp` is cap-gated out of its alphabet and no
transition sequence boots a connector at all — the reason this dedicated
harness exists. That is the COVERAGE half; the ENVIRONMENT half is that the
harness ran in no gate.

## Remedy

`EntityConfig::to_type_definition` now appends `FieldSchema::overflow_pair()` to
the authored schema, so every sidecar-mirrored type declares a home for the
engine's stamp and the mirror table's DDL gains the two columns. A schema that
declares `properties` itself is left as authored, so the type's own `value_kind`
decides and the existing check names it precisely when it is the wrong one.

An entity's columns are a `MirrorSchema` (`crates/holon-mcp-client/src/
mcp_sidecar.rs`), and `MirrorSchema::parse` is the only producer of one. Both
legs go through it: `McpSidecar::from_yaml` for an authored schema, and
`absorb_discovered_entity` for the columns a server advertises. A discovered
schema is parsed BEFORE anything is written, so a refused resource template
costs that one entity — the failure is logged naming the server, the template
and the column, and the rest of the integration connects.

Open residual, pre-dating this bug: `QueryableCache::initialize_schema` issues
`CREATE TABLE IF NOT EXISTS` and never reconciles an existing mirror table with
a changed sidecar schema, so a vault whose mirror table was created before this
fix keeps a table without the pair. The same hole meets any sidecar author who
adds a column to `schema:`; the house pattern for closing it is the
`PRAGMA table_info` + `ALTER TABLE … ADD COLUMN` reconciliation in
`crates/holon-turso/src/schema_modules.rs:940-980`.
