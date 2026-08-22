//! A sidecar cache table must never be a rowid-alias table.
//!
//! The vtable writes fetched rows back with a chunked `INSERT OR REPLACE`
//! (`mcp_vtable.rs`), on every refresh, deliberately rewriting rows that have
//! not changed — its own doc comment calls that idempotent. Sidecar-declared
//! views over those cache tables become real materialized views at connect
//! (`reconcile_sidecar_views`), so each cache table is a matview base.
//!
//! On this Turso fork, one `INSERT OR REPLACE` that overwrites an existing row
//! with an UNCHANGED value corrupts every matview over that table — but ONLY
//! when the table is a rowid-alias table, i.e. it has a single `INTEGER PRIMARY
//! KEY` column. The row's weight goes to -1 and it silently disappears from the
//! view; the read does not error. Measured across projection, filter,
//! aggregate, INNER JOIN and LEFT JOIN shapes in
//! `crates/holon-turso/tests/replace_into_matview_base.rs`.
//!
//! Every shipped sidecar declares TEXT primary keys today, which is the only
//! reason the timer-driven writeback is not corrupting live data. That is a
//! property of the YAML, not of the code — so it is asserted here. A future
//! sidecar declaring `sql_type: INTEGER` on its single primary key would give
//! Holon this bug with no code change and no other test noticing.
//!
//! `queryable_cache::generate_create_table_sql_with_change_origin` decides the
//! shape: exactly one `primary_key` field emits an inline `<col> <type> PRIMARY
//! KEY`, and only `INTEGER` there aliases the rowid. Two or more primary-key
//! fields emit a table-level `PRIMARY KEY (a, b)`, which never aliases the
//! rowid, so composite keys are safe whatever their types.

use holon_mcp_client::mcp_sidecar::McpSidecar;

/// Every YAML under `assets/integrations/`, as (file name, parsed sidecar).
fn shipped_sidecars() -> Vec<(String, McpSidecar)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/integrations");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("sidecar file name")
            .to_string();
        let yaml = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let sidecar = McpSidecar::from_yaml(&yaml)
            .unwrap_or_else(|e| panic!("parse shipped sidecar {name}: {e}"));
        out.push((name, sidecar));
    }
    assert!(
        !out.is_empty(),
        "no sidecars found under {} — the test would pass vacuously",
        dir.display()
    );
    out
}

/// Does this entity's cache table become a rowid-alias table? Only a LONE
/// primary key becomes an inline `<col> <type> PRIMARY KEY`, and only `INTEGER`
/// there aliases the rowid; two or more primary-key fields emit a table-level
/// `PRIMARY KEY (a, b)`, which never does.
fn lone_integer_primary_key(
    entity: &holon_mcp_client::mcp_sidecar::EntityConfig,
) -> Option<&holon_api::entity::FieldSchema> {
    let pks: Vec<&holon_api::entity::FieldSchema> =
        entity.schema.iter().filter(|f| f.primary_key).collect();
    match pks.as_slice() {
        [pk] if pk.sql_type.trim().eq_ignore_ascii_case("INTEGER") => Some(pk),
        _ => None,
    }
}

/// The hazard is the CONJUNCTION: a rowid-alias cache table that the vtable
/// writeback rewrites with `INSERT OR REPLACE`. Either half alone is safe.
///
/// The sync path is the reason the halves must be separated: `QueryableCache`
/// upserts with `INSERT ... ON CONFLICT DO UPDATE`, which is measured correct
/// on a rowid table with an unchanged value
/// (`on_conflict_do_update_with_unchanged_value_keeps_a_rowid_table_matview_correct`).
/// So a sync-only entity may hold an INTEGER primary key safely — as
/// `jsonplaceholder`'s `jp_posts` does today.
#[test]
fn no_write_through_cache_table_is_a_rowid_alias_table() {
    let mut offenders = Vec::new();
    let mut checked = 0usize;

    for (file, sidecar) in shipped_sidecars() {
        for (entity_name, entity) in &sidecar.entities {
            let write_through = entity.vtable.as_ref().is_some_and(|v| v.write_through);
            if entity.schema.is_empty() || !write_through {
                continue;
            }
            checked += 1;
            if let Some(pk) = lone_integer_primary_key(entity) {
                offenders.push(format!(
                    "{file}: entity `{entity_name}` has vtable.write_through AND field `{}` is \
                     `INTEGER PRIMARY KEY`",
                    pk.name
                ));
            }
        }
    }

    assert!(
        checked > 0,
        "no shipped entity has both a schema and vtable.write_through — this assertion would be \
         vacuous, so the sidecars or the field names have moved"
    );
    assert!(
        offenders.is_empty(),
        "a write-through cache table is a rowid-alias table. The hazard is the CONJUNCTION \
         `rowid-alias matview base + REPLACE writer`: the vtable writeback rewrites unchanged \
         rows ON A TIMER with INSERT OR REPLACE, which on this Turso fork silently drops rows \
         from every matview over that table — the read does not even error (see \
         crates/holon-turso/tests/replace_into_matview_base.rs; engine fix PR #8463). Declare \
         the key as TEXT, use a composite primary key, or stop writing that table with \
         REPLACE.\n  {}",
        offenders.join("\n  ")
    );
}

/// A rowid-alias cache table is safe ONLY while nothing REPLACEs into it, which
/// is a fragile thing to depend on silently. This records the ones that exist
/// so the dependency is visible: if a future change gives one of these a
/// REPLACE writer, the sibling assertion above is what must catch it.
#[test]
fn rowid_alias_cache_tables_are_documented() {
    let found: Vec<String> = shipped_sidecars()
        .iter()
        .flat_map(|(file, sidecar)| {
            sidecar
                .entities
                .iter()
                .filter_map(|(name, entity)| {
                    lone_integer_primary_key(entity).map(|_| format!("{file}:{name}"))
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // Sorted so the comparison does not depend on HashMap iteration order.
    let mut found = found;
    found.sort();
    assert_eq!(
        found,
        vec!["jsonplaceholder.yaml:jp_posts".to_string()],
        "the set of rowid-alias cache tables changed. Each one is safe only while its writer is \
         an ON CONFLICT upsert rather than INSERT OR REPLACE; confirm that for any new entry \
         before updating this list."
    );
}
