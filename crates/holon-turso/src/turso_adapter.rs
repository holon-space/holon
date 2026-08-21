//! TursoAdapter: derives the **Turso serialization** of a datatype from its
//! [`TypeDefinition`].
//!
//! Turso is one format adapter among many (Loro, org, CSV, ...); none is the
//! source of truth. The datatype's `TypeDefinition` is primary, and this
//! adapter emits every Turso artifact for a free-standing type from it:
//!
//! - a raw base table `<name>_raw` (the write surface), and
//! - a read matview `<name>` chained on it (the query/reactivity surface, plus
//!   the increment-3 seam for SQL-planted computed columns).
//!
//! This is exactly block's `block_raw` -> `block` split, generalized. Nothing
//! is hand-written per TYPE: `person.yaml` + this generic adapter produce
//! person's whole Turso footprint, and deleting the generated artifacts and
//! re-registering the type reproduces them byte-identically (the
//! regeneration-idempotence law, pinned by tests).

use std::sync::Arc;

use async_trait::async_trait;
use holon_api::TypeDefinition;
use holon_core::storage::Resource;
use holon_core::storage::Result;
use holon_core::storage::StorageError;

use super::dynamic_schema_module::DynamicSchemaModule;
use super::matview_manager::reconcile_named_view;
use super::schema_module::SchemaModule;
use super::turso::DbHandle;

/// Suffix separating a type's raw write table from its read matview. Writes
/// target `<name>_raw`; `SELECT ... FROM <name>` / PRQL `from <name>` read the
/// `<name>` matview — the same split block uses (`block_raw` -> `block`).
pub const RAW_TABLE_SUFFIX: &str = "_raw";

/// A SQL-planted derived column for a type's matview (increment-3 seam). Each
/// entry appends `({sql}) AS {name}` to the matview SELECT, so Turso's IVM
/// maintains the value O(delta) alongside the row. Empty today.
#[derive(Debug, Clone)]
pub struct MatviewComputedColumn {
    pub name: String,
    pub sql: String,
}

/// The inventory of Turso objects a single [`TypeDefinition`] owns. Returned by
/// [`TursoAdapter::register`] and consumed by [`TursoAdapter::teardown`], which
/// together form the migrate primitive: a type's whole Turso footprint is
/// enumerable, so it can be dropped and regenerated without touching anything
/// else in the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TursoArtifacts {
    /// The base table rows are written to (`<name>_raw`).
    pub raw_table: String,
    /// The read matview (`<name>`).
    pub matview: String,
    /// The type's contributions to the generated PRQL stdlib. Empty for a
    /// free-standing type, which needs no derived relations.
    pub prql_stdlib_entries: Vec<String>,
}

/// SQLite's reserved words. Only the ones that can begin or terminate a
/// statement clause matter for identifier position, but the full list costs
/// nothing and avoids arguing about which subset is safe.
const SQL_KEYWORDS: &[&str] = &[
    "abort",
    "action",
    "add",
    "after",
    "all",
    "alter",
    "always",
    "analyze",
    "and",
    "as",
    "asc",
    "attach",
    "autoincrement",
    "before",
    "begin",
    "between",
    "by",
    "cascade",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "commit",
    "conflict",
    "constraint",
    "create",
    "cross",
    "current",
    "current_date",
    "current_time",
    "current_timestamp",
    "database",
    "default",
    "deferrable",
    "deferred",
    "delete",
    "desc",
    "detach",
    "distinct",
    "do",
    "drop",
    "each",
    "else",
    "end",
    "escape",
    "except",
    "exclude",
    "exclusive",
    "exists",
    "explain",
    "fail",
    "filter",
    "first",
    "following",
    "for",
    "foreign",
    "from",
    "full",
    "generated",
    "glob",
    "group",
    "groups",
    "having",
    "if",
    "ignore",
    "immediate",
    "in",
    "index",
    "indexed",
    "initially",
    "inner",
    "insert",
    "instead",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "key",
    "last",
    "left",
    "like",
    "limit",
    "match",
    "materialized",
    "natural",
    "no",
    "not",
    "nothing",
    "notnull",
    "null",
    "nulls",
    "of",
    "offset",
    "on",
    "or",
    "order",
    "others",
    "outer",
    "over",
    "partition",
    "plan",
    "pragma",
    "preceding",
    "primary",
    "query",
    "raise",
    "range",
    "recursive",
    "references",
    "regexp",
    "reindex",
    "release",
    "rename",
    "replace",
    "restrict",
    "returning",
    "right",
    "rollback",
    "row",
    "rows",
    "savepoint",
    "select",
    "set",
    "table",
    "temp",
    "temporary",
    "then",
    "ties",
    "to",
    "transaction",
    "trigger",
    "unbounded",
    "union",
    "unique",
    "update",
    "using",
    "vacuum",
    "values",
    "view",
    "virtual",
    "when",
    "where",
    "window",
    "with",
    "without",
];

/// Whether an identifier collides with a SQL keyword (case-insensitively).
///
/// Public so generators can AVOID emitting one: [`TursoAdapter::register`]
/// rejects keyword identifiers, and a generator that drew them would spend its
/// cases bouncing off that error instead of exercising the adapter.
pub fn is_sql_keyword(ident: &str) -> bool {
    let lowered = ident.to_ascii_lowercase();
    SQL_KEYWORDS.contains(&lowered.as_str())
}

/// Derives a type's Turso artifacts from its [`TypeDefinition`].
pub struct TursoAdapter;

impl TursoAdapter {
    /// The raw base-table name a type's rows are written to.
    pub fn raw_table_name(type_def: &TypeDefinition) -> String {
        format!("{}{RAW_TABLE_SUFFIX}", type_def.name)
    }

    /// The read-matview name — the type name itself. `from <name>` (PRQL) and
    /// `SELECT ... FROM <name>` resolve here.
    pub fn matview_name(type_def: &TypeDefinition) -> String {
        type_def.name.clone()
    }

    /// The write schema: the type renamed onto its raw base table, carrying
    /// its PERSISTED fields only. A free-standing type keeps
    /// `id_references: None`, so no FK is emitted.
    ///
    /// Computed fields are deliberately absent: they are derived, not stored,
    /// and materialize on the matview through
    /// [`MatviewComputedColumn`] instead. Emitting one here would create a
    /// NOT NULL column that no writer ever supplies.
    pub fn raw_type_def(type_def: &TypeDefinition) -> TypeDefinition {
        TypeDefinition {
            name: Self::raw_table_name(type_def),
            fields: type_def
                .persistent_fields()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            ..type_def.clone()
        }
    }

    /// The matview SELECT: every declared field projected verbatim from the raw
    /// table.
    pub fn matview_select(type_def: &TypeDefinition) -> String {
        Self::matview_select_with_computed(type_def, &[])
    }

    /// [`Self::matview_select`] extended with SQL-planted computed columns
    /// (increment-3 seam). `computed` is empty on every path today.
    pub fn matview_select_with_computed(
        type_def: &TypeDefinition,
        computed: &[MatviewComputedColumn],
    ) -> String {
        let persisted = type_def.persistent_fields();
        assert!(
            !persisted.is_empty(),
            "Cannot derive a matview for type '{}' with no persisted fields.",
            type_def.name
        );
        let raw = Self::raw_table_name(type_def);
        let mut columns: Vec<String> = persisted
            .iter()
            .map(|f| format!("\"{}\"", f.name))
            .collect();
        for c in computed {
            columns.push(format!("({}) AS {}", c.sql, c.name));
        }
        format!("SELECT {} FROM \"{raw}\"", columns.join(", "))
    }

    /// The generated PRQL stdlib fragment: a type's DERIVED relations
    /// (`children`/`siblings`, for a hierarchical type). A free-standing,
    /// non-hierarchical type (`id_references: None`) contributes none —
    /// `from <name>` resolves straight to the matview, exactly as `from block`
    /// does. Empty string = no fragment. Hierarchical derived relations land
    /// with the first hierarchical non-block type (or block's own migration),
    /// at which point this fragment is composed into the query stdlib.
    pub fn prql_stdlib_fragment(type_def: &TypeDefinition) -> String {
        // Hierarchy is an opt-in capability (BG-3). Increment 1 only serializes
        // free-standing types, which have no parent axis and thus no derived
        // relations.
        let _ = type_def;
        String::new()
    }

    /// The ordered [`SchemaModule`]s that serialize a type into Turso: raw
    /// table first, then the read matview chained on it. Running them in this
    /// order satisfies the matview's `requires(<name>_raw)` gate.
    pub fn schema_modules(type_def: &TypeDefinition) -> Vec<Arc<dyn SchemaModule>> {
        vec![
            DynamicSchemaModule::arc(Self::raw_type_def(type_def)),
            Arc::new(DerivedMatviewModule::new(type_def.clone())),
        ]
    }

    /// Create a type's whole Turso footprint and report what was created.
    ///
    /// Each module's resources are marked available before the next runs, so
    /// the matview's `requires(<name>_raw)` DDL gate is satisfied in-order.
    pub async fn register(
        type_def: &TypeDefinition,
        db_handle: &DbHandle,
    ) -> Result<TursoArtifacts> {
        Self::reject_keyword_identifiers(type_def)?;
        for module in Self::schema_modules(type_def) {
            module.ensure_schema(db_handle).await.map_err(|e| {
                StorageError::DatabaseError(format!(
                    "TursoAdapter::register('{}'): ensure_schema failed for module '{}': {e}",
                    type_def.name,
                    module.name()
                ))
            })?;
            module.initialize_data(db_handle).await.map_err(|e| {
                StorageError::DatabaseError(format!(
                    "TursoAdapter::register('{}'): initialize_data failed for module '{}': {e}",
                    type_def.name,
                    module.name()
                ))
            })?;
            db_handle
                .mark_available(module.provides())
                .await
                .map_err(|e| {
                    StorageError::DatabaseError(format!(
                        "TursoAdapter::register('{}'): mark_available failed for module '{}': {e}",
                        type_def.name,
                        module.name()
                    ))
                })?;
        }

        let fragment = Self::prql_stdlib_fragment(type_def);
        Ok(TursoArtifacts {
            raw_table: Self::raw_table_name(type_def),
            matview: Self::matview_name(type_def),
            prql_stdlib_entries: if fragment.is_empty() {
                Vec::new()
            } else {
                vec![fragment]
            },
        })
    }

    /// Reject a type whose table or column name is a SQL keyword.
    ///
    /// TEMPORARY, and lifted by the engine fix. Quoting is the normal way to
    /// use a keyword as an identifier, but a quoted table name in a write
    /// bypasses IVM dependency tracking in our Turso fork, so `DbHandle`
    /// refuses quoted writes — which leaves a keyword-named type
    /// unwritable. Catching it HERE, at declaration, turns what would be a
    /// syntax error at the first write (runtime, far from the cause) into a
    /// typed error at the boundary.
    fn reject_keyword_identifiers(type_def: &TypeDefinition) -> Result<()> {
        let mut offenders: Vec<&str> = Vec::new();
        if is_sql_keyword(&type_def.name) {
            offenders.push(&type_def.name);
        }
        offenders.extend(
            type_def
                .persistent_fields()
                .into_iter()
                .map(|f| f.name.as_str())
                .filter(|n| is_sql_keyword(n)),
        );
        if offenders.is_empty() {
            return Ok(());
        }
        Err(StorageError::DatabaseError(format!(
            "type '{}' declares SQL keyword identifier(s) {offenders:?}: until our Turso fork's \
             IVM normalizes quoted identifiers, keyword-named types cannot be safely written — \
             quoting them silently stops matview maintenance, and not quoting them is a syntax \
             error. Rename the type or field, or wait for the engine fix.",
            type_def.name
        )))
    }

    /// Drop everything [`Self::register`] created, matview before base table so
    /// the dependency direction is respected.
    pub async fn teardown(artifacts: &TursoArtifacts, db_handle: &DbHandle) -> Result<()> {
        for ddl in [
            format!("DROP VIEW IF EXISTS \"{}\"", artifacts.matview),
            format!("DROP TABLE IF EXISTS \"{}\"", artifacts.raw_table),
        ] {
            db_handle.execute_ddl(&ddl).await.map_err(|e| {
                StorageError::DatabaseError(format!("TursoAdapter::teardown: '{ddl}' failed: {e}"))
            })?;
        }
        Ok(())
    }
}

/// The read-matview [`SchemaModule`] for a type: `CREATE MATERIALIZED VIEW
/// <name> AS SELECT <fields> FROM <name>_raw`, reconciled idempotently.
pub struct DerivedMatviewModule {
    type_def: TypeDefinition,
}

impl DerivedMatviewModule {
    pub fn new(type_def: TypeDefinition) -> Self {
        Self { type_def }
    }
}

#[async_trait]
impl SchemaModule for DerivedMatviewModule {
    fn name(&self) -> &str {
        // The matview name == the type name.
        &self.type_def.name
    }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema(TursoAdapter::matview_name(&self.type_def))]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema(TursoAdapter::raw_table_name(
            &self.type_def,
        ))]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        let view = TursoAdapter::matview_name(&self.type_def);
        let select = TursoAdapter::matview_select(&self.type_def);
        tracing::info!("[DerivedMatviewModule] Reconciling '{view}' matview: {select}");
        reconcile_named_view(db_handle, &view, &select)
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to reconcile matview '{view}': {e}"))
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use holon_api::FieldSchema;

    use super::*;

    fn person_td() -> TypeDefinition {
        TypeDefinition {
            source: holon_api::entity::TypeSource::PreConfigured,
            ..TypeDefinition::new(
                "person",
                vec![
                    FieldSchema::new("id", "TEXT").primary_key(),
                    FieldSchema::new("email", "TEXT").nullable(),
                    FieldSchema::new("role", "TEXT").nullable(),
                ],
            )
        }
    }

    #[test]
    fn derives_raw_table_and_matview_names() {
        let td = person_td();
        assert_eq!(TursoAdapter::raw_table_name(&td), "person_raw");
        assert_eq!(TursoAdapter::matview_name(&td), "person");
        assert_eq!(TursoAdapter::raw_type_def(&td).name, "person_raw");
        // Free-standing: no FK in the raw DDL.
        assert!(
            !TursoAdapter::raw_type_def(&td)
                .to_create_table_sql()
                .contains("REFERENCES"),
            "free-standing raw table must not emit a FK"
        );
    }

    #[test]
    fn matview_select_projects_every_field_from_raw() {
        let td = person_td();
        assert_eq!(
            TursoAdapter::matview_select(&td),
            "SELECT \"id\", \"email\", \"role\" FROM \"person_raw\""
        );
    }

    // A computed field is DERIVED, not stored. If it leaked into the raw write
    // table it would become a NOT NULL column no writer ever supplies, and
    // every insert would fail — which is exactly what person's profile-injected
    // `display_name` did before the adapter filtered on lifetime.
    #[test]
    fn computed_fields_stay_out_of_the_write_table_and_matview() {
        let computed: FieldSchema = serde_json::from_value(serde_json::json!({
            "name": "display_name",
            "sql_type": "TEXT",
            "lifetime": {"computed": {"expr": "email"}},
        }))
        .expect("computed FieldSchema");

        let mut td = person_td();
        td.fields.push(computed);

        let raw_sql = TursoAdapter::raw_type_def(&td).to_create_table_sql();
        assert!(
            !raw_sql.contains("display_name"),
            "computed field must not become a write-table column; DDL was:\n{raw_sql}"
        );
        assert_eq!(
            TursoAdapter::matview_select(&td),
            "SELECT \"id\", \"email\", \"role\" FROM \"person_raw\""
        );
    }

    #[test]
    fn free_standing_type_contributes_no_prql_fragment() {
        assert_eq!(TursoAdapter::prql_stdlib_fragment(&person_td()), "");
    }

    #[test]
    fn schema_modules_are_raw_then_matview_with_matching_deps() {
        let td = person_td();
        let modules = TursoAdapter::schema_modules(&td);
        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0].provides(), vec![Resource::schema("person_raw")]);
        assert_eq!(modules[1].provides(), vec![Resource::schema("person")]);
        assert_eq!(modules[1].requires(), vec![Resource::schema("person_raw")]);
    }

    // The regeneration-idempotence law: deleting the generated matview and
    // re-registering the type reproduces it byte-identically, and the whole
    // derivation is a pure function of the TypeDefinition.
    #[tokio::test]
    async fn regeneration_is_idempotent_end_to_end() {
        use std::collections::HashMap;

        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let td = person_td();

        let inv1 = TursoAdapter::register(&td, &handle)
            .await
            .expect("register");
        assert_eq!(inv1.raw_table, "person_raw");
        assert_eq!(inv1.matview, "person");
        assert!(
            inv1.prql_stdlib_entries.is_empty(),
            "a free-standing type contributes no stdlib entries"
        );

        let matview_sql_1 = handle
            .query(
                "SELECT sql FROM sqlite_master WHERE type='view' AND name='person'",
                HashMap::new(),
            )
            .await
            .expect("query matview sql");
        assert_eq!(matview_sql_1.len(), 1, "person matview must exist");

        // Tear the whole footprint down through the disposer, then re-register:
        // both the inventory and the matview DDL must come back identical
        // (the derivation is a pure function of the TypeDefinition).
        TursoAdapter::teardown(&inv1, &handle)
            .await
            .expect("teardown");
        let gone = handle
            .query(
                "SELECT name FROM sqlite_master WHERE name IN ('person', 'person_raw')",
                HashMap::new(),
            )
            .await
            .expect("query torn-down objects");
        assert!(gone.is_empty(), "teardown must remove the whole footprint");

        let inv2 = TursoAdapter::register(&td, &handle)
            .await
            .expect("re-register");
        assert_eq!(inv1, inv2, "regenerated inventory must be identical");

        let matview_sql_2 = handle
            .query(
                "SELECT sql FROM sqlite_master WHERE type='view' AND name='person'",
                HashMap::new(),
            )
            .await
            .expect("query matview sql");
        assert_eq!(
            matview_sql_1.first().and_then(|r| r.get("sql")),
            matview_sql_2.first().and_then(|r| r.get("sql")),
            "regenerated matview DDL must be byte-identical"
        );

        handle.shutdown().await.expect("shutdown");
    }

    // The adapter's matview must be IVM-maintained for writes that go through
    // `DbHandle::execute_values` (the path every non-`StorageBackend` writer
    // takes), not only for `StorageBackend::insert`.
    //
    // HAZARD this pins, BOTH LEGS: writing `INSERT INTO "person_raw" ...` — the
    // table name DOUBLE-QUOTED — makes Turso's IVM silently stop maintaining
    // every matview over that table, while the insert still reports success and
    // the row really is in the base table.
    //
    // The quoted leg runs through `execute_unguarded` ON PURPOSE. The guard now
    // rejects quoted writes at both entry points, so without that bypass the
    // broken engine path is unreachable and this test would silently stop
    // demonstrating anything — passing whether or not the defect still exists.
    //
    // WHEN THE QUOTED LEG FAILS, the fork has been fixed: delete the guard,
    // `execute_unguarded`, and that assertion together.
    #[tokio::test]
    async fn matview_is_ivm_maintained_for_execute_values_writes() {
        use std::collections::HashMap;

        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let td = person_td();
        TursoAdapter::register(&td, &handle)
            .await
            .expect("register");

        handle
            .execute_values(
                "INSERT INTO person_raw (id, email, role) VALUES (?, ?, ?)",
                vec![
                    holon_api::Value::String("person-0".into()),
                    holon_api::Value::String("bcdpa".into()),
                    holon_api::Value::String("a".into()),
                ],
            )
            .await
            .expect("insert via execute_values");

        let raw = handle
            .query("SELECT id FROM person_raw", HashMap::new())
            .await
            .expect("query raw");
        assert_eq!(raw.len(), 1, "row must be in the raw write table");

        let via_matview = handle
            .query("SELECT id, email, role FROM person", HashMap::new())
            .await
            .expect("query matview");
        assert_eq!(
            via_matview.len(),
            1,
            "IVM must maintain the person matview for an execute_values write"
        );

        // The B leg: the SAME insert with the table name quoted, driven past the
        // guard. The row lands in the base table and the matview does NOT move —
        // that divergence IS the engine defect.
        handle
            .execute_unguarded(
                "INSERT INTO \"person_raw\" (id, email, role) VALUES (?, ?, ?)",
                vec![
                    holon_api::Value::String("person-1".into()),
                    holon_api::Value::String("quoted".into()),
                    holon_api::Value::String("b".into()),
                ],
            )
            .await
            .expect("unguarded quoted insert reaches the engine");

        let raw_after = handle
            .query("SELECT id FROM person_raw", HashMap::new())
            .await
            .expect("query raw");
        assert_eq!(raw_after.len(), 2, "the quoted write DID land in the table");

        let matview_after = handle
            .query("SELECT id FROM person", HashMap::new())
            .await
            .expect("query matview");
        assert_eq!(
            matview_after.len(),
            1,
            "ENGINE DEFECT still present: a quoted write must leave the matview stale at 1 row. \
             If this is 2, the fork now normalizes quoted identifiers — delete the guard, \
             execute_unguarded, and this leg."
        );

        handle.shutdown().await.expect("shutdown");
    }

    // The batch path is the one the guard was BUILT for — QueryableCache's
    // change-stream writer submits through `transaction`, not `execute`. A
    // guard on `execute` alone would let the highest-volume writer route
    // straight around it.
    #[tokio::test]
    async fn a_quoted_write_inside_a_transaction_batch_rejects_the_whole_batch() {
        use std::collections::HashMap;

        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let td = person_td();
        TursoAdapter::register(&td, &handle).await.expect("register");

        let text = |v: &str| turso::Value::Text(v.to_string());
        let err = handle
            .transaction(vec![
                (
                    "INSERT INTO person_raw (id, email) VALUES (?, ?)".to_string(),
                    vec![text("ok-0"), text("a")],
                ),
                (
                    // The offender, sitting SECOND — a guard that only checked
                    // the first statement would miss it.
                    "INSERT INTO \"person_raw\" (id, email) VALUES (?, ?)".to_string(),
                    vec![text("bad-1"), text("b")],
                ),
                (
                    "INSERT INTO person_raw (id, email) VALUES (?, ?)".to_string(),
                    vec![text("ok-2"), text("c")],
                ),
            ])
            .await
            .expect_err("a quoted table identifier anywhere in the batch must reject it");

        let msg = err.to_string();
        assert!(
            msg.contains("bypass IVM dependency tracking"),
            "the error must name the defect; got: {msg}"
        );
        assert!(
            msg.contains("statement 1 of the transaction batch"),
            "the error must name WHICH statement offended; got: {msg}"
        );

        // Screened BEFORE anything ran: the two legal statements must not have
        // landed either, or the guard would be leaving partial writes behind.
        let rows = handle
            .query("SELECT id FROM person_raw", HashMap::new())
            .await
            .expect("query raw");
        assert!(
            rows.is_empty(),
            "a rejected batch must execute NOTHING; found {} row(s)",
            rows.len()
        );

        handle.shutdown().await.expect("shutdown");
    }

    // A keyword-named type is rejected at DECLARATION, not at the first write.
    // The restriction is temporary: it exists only because a keyword identifier
    // must be quoted, and a quoted write bypasses IVM in our fork.
    #[tokio::test]
    async fn a_keyword_named_type_is_rejected_at_registration() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let mut td = person_td();
        td.name = "order".to_string();

        let err = TursoAdapter::register(&td, &handle)
            .await
            .expect_err("a SQL-keyword type name must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("keyword-named types cannot be safely written"),
            "the error must name the limitation; got: {msg}"
        );
        assert!(
            msg.contains("order"),
            "the error must name the offending identifier; got: {msg}"
        );

        // The rejection happens BEFORE any DDL runs — nothing was created.
        let created = handle
            .query(
                "SELECT name FROM sqlite_master WHERE name IN ('order', 'order_raw')",
                std::collections::HashMap::new(),
            )
            .await
            .expect("query sqlite_master");
        assert!(created.is_empty(), "a rejected type must leave no artifacts");

        // A keyword FIELD name is rejected the same way.
        let mut td2 = person_td();
        td2.fields.push(FieldSchema::new("select", "TEXT").nullable());
        let err2 = TursoAdapter::register(&td2, &handle)
            .await
            .expect_err("a SQL-keyword field name must be rejected");
        assert!(err2.to_string().contains("select"));

        handle.shutdown().await.expect("shutdown");
    }

    // The quoted form is REJECTED at BOTH DbHandle write entry points —
    // `execute` and `transaction`. Loud beats silent: normalizing the SQL
    // instead would route around the fork bug and hide the pressure to fix it.
    //
    // (An earlier version of this comment claimed the hazard "cannot reach the
    // engine at all". That was FALSE while `transaction` was unguarded — the
    // batch writers went straight past the check.)
    #[tokio::test]
    async fn a_quoted_write_is_rejected_before_it_can_desync_a_matview() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let td = person_td();
        TursoAdapter::register(&td, &handle)
            .await
            .expect("register");

        let err = handle
            .execute_values(
                "INSERT INTO \"person_raw\" (id, email, role) VALUES (?, ?, ?)",
                vec![
                    holon_api::Value::String("person-0".into()),
                    holon_api::Value::String("bcdpa".into()),
                    holon_api::Value::String("a".into()),
                ],
            )
            .await
            .expect_err("a double-quoted write target must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("bypass IVM dependency tracking"),
            "the error must name the defect; got: {msg}"
        );
        assert!(
            msg.contains("matview_is_ivm_maintained_for_execute_values_writes"),
            "the error must point at the pin test; got: {msg}"
        );

        handle.shutdown().await.expect("shutdown");
    }

    // A free-standing typed entity lives in its own table and NEVER appears in
    // any block table — the datatype-axis identity, proven at the storage layer.
    #[tokio::test]
    async fn free_standing_entity_never_lands_in_a_block_table() {
        use std::collections::HashMap;

        use crate::turso::TursoBackend;

        let (backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let td = person_td();
        for module in TursoAdapter::schema_modules(&td) {
            module.ensure_schema(&handle).await.expect("ensure_schema");
        }

        // Insert through the write schema (raw table).
        let mut row: holon_api::StorageEntity = HashMap::new();
        row.insert("id".into(), holon_api::Value::String("alice".into()));
        row.insert(
            "email".into(),
            holon_api::Value::String("alice@example.com".into()),
        );
        holon_core::storage::StorageBackend::insert(
            &backend,
            &TursoAdapter::raw_type_def(&td),
            row,
        )
        .await
        .expect("insert person row");

        // The row is readable through the matview.
        let via_matview = handle
            .query("SELECT id, email FROM person", HashMap::new())
            .await
            .expect("query person matview");
        assert_eq!(via_matview.len(), 1, "person row visible via matview");

        // And there is no block table carrying it.
        let block_tables = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'block%'",
                HashMap::new(),
            )
            .await
            .expect("query block tables");
        assert!(
            block_tables.is_empty(),
            "no block table should exist in a person-only schema"
        );

        handle.shutdown().await.expect("shutdown");
    }
}
