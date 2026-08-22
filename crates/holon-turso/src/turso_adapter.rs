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
        Self::reject_non_lowercase_name(type_def)?;
        Self::reject_non_identifier_name(type_def)?;
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
    /// Quoting is the normal way to use a keyword as an identifier, and this
    /// adapter does not spell one: [`reconcile_named_view`] interpolates the
    /// matview name UNQUOTED into `CREATE MATERIALIZED VIEW`, so a type named
    /// `order` dies with `near "order": syntax error` and never gets a read
    /// surface — while writes to `order_raw` succeed, leaving the type
    /// half-created. `SqlOperationProvider`'s INSERT/UPDATE/DELETE interpolate
    /// the table name unquoted too. Catching it HERE, at declaration, turns a
    /// DDL syntax error raised deep inside `ensure_schema` into a typed error
    /// naming the type that caused it.
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
            "type '{}' declares SQL keyword identifier(s) {offenders:?}: this adapter \
             interpolates the name unquoted into `CREATE MATERIALIZED VIEW` and into every \
             INSERT/UPDATE/DELETE, so a keyword reaches the SQL parser as a syntax error and the \
             type never gets a read surface. Rename the type or field.",
            type_def.name
        )))
    }

    /// Reject a type whose name is not already lowercase.
    ///
    /// The Turso serialization CANONICALIZES a type's names to lowercase: a
    /// type declared `Person` reaches the schema as table `person_raw`, view
    /// `person` and state table `__turso_internal_dbsp_state_v1_person`. So a
    /// non-lowercase type name is not representable in storage — it is a
    /// spelling the schema never keeps.
    ///
    /// That makes it worse than cosmetic. `TypeRegistry` keys its map on the
    /// raw `String` and `EntityName` compares case-sensitively, so `Person` and
    /// `person` are two distinct types everywhere above storage and ONE table
    /// and ONE matview inside it — and because the declared case is discarded
    /// before `sqlite_master` sees it, nothing downstream can tell the
    /// collision from an ordinary re-registration. Refusing the non-canonical
    /// spelling at declaration is what keeps that state unrepresentable.
    ///
    /// This is NOT a limit of the engine's identifier handling: writes and
    /// reads that SPELL the name in any case resolve to the same table (pinned
    /// by [`mixed_case_sql_resolves_to_the_lowercase_type`]). It is a limit of
    /// letting two type names differ only by a distinction storage discards.
    fn reject_non_lowercase_name(type_def: &TypeDefinition) -> Result<()> {
        if type_def.name == type_def.name.to_lowercase() {
            return Ok(());
        }
        Err(StorageError::DatabaseError(format!(
            "type '{}' is not lowercase: the Turso serialization canonicalizes names to \
             lowercase, so this type would reach the schema as '{}' — the declared spelling is \
             discarded. A second type named '{}' would then share one table and one matview with \
             it, undetectably. Rename the type to '{}'.",
            type_def.name,
            type_def.name.to_lowercase(),
            type_def.name.to_lowercase(),
            type_def.name.to_lowercase()
        )))
    }

    /// Reject a name this adapter cannot spell as a bare SQL identifier
    /// (`[a-z][a-z0-9_]*`).
    ///
    /// This is a limit of the TURSO SERIALIZATION, not a rule about type names
    /// in general — `TypeRegistry` deliberately carries hyphenated types, and
    /// a hyphen is a perfectly good URI scheme. It is the DDL that cannot take
    /// one: the name is interpolated UNQUOTED into `CREATE TABLE`, so `gen-1`
    /// arrives at the parser as a syntax error.
    ///
    /// Quoting is what would let this adapter carry the wider alphabet, and
    /// this adapter does not quote — the same limit
    /// [`Self::reject_keyword_identifiers`] names. Even if it did, a leading
    /// digit or an embedded space can never be a valid URI scheme, so those
    /// shapes would stay refused here (else they reach `EntityName::new`'s
    /// debug_assert). The rule also keeps `EntityName`'s `_`→`-` fold
    /// injective over serialized types (`gen-1` and `gen_1` both canonicalize
    /// to `gen-1`).
    ///
    /// Case is handled one rule earlier, by
    /// [`Self::reject_non_lowercase_name`], so by the time a name reaches this
    /// check it is already lowercase.
    fn reject_non_identifier_name(type_def: &TypeDefinition) -> Result<()> {
        let mut chars = type_def.name.chars();
        let shaped = matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if shaped {
            return Ok(());
        }
        Err(StorageError::DatabaseError(format!(
            "type '{}' cannot be serialized to Turso: its raw table name is interpolated unquoted \
             into DDL, so the name must match [a-z][a-z0-9_]* — a hyphen, a space, or a leading \
             digit reaches the SQL parser as a syntax error. The type itself may keep any name \
             the registry accepts; only its Turso serialization is refused. Rename the type for \
             storage.",
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
    // The second leg spells the table name DOUBLE-QUOTED. The engine resolves a
    // write's table through the schema before keying the IVM delta, so the
    // quoted spelling maintains the matview exactly like the bare one.
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

        handle
            .execute_values(
                "INSERT INTO \"person_raw\" (id, email, role) VALUES (?, ?, ?)",
                vec![
                    holon_api::Value::String("person-1".into()),
                    holon_api::Value::String("quoted".into()),
                    holon_api::Value::String("b".into()),
                ],
            )
            .await
            .expect("quoted insert");

        let raw_after = handle
            .query("SELECT id FROM person_raw", HashMap::new())
            .await
            .expect("query raw");
        assert_eq!(raw_after.len(), 2, "the quoted write landed in the table");

        let matview_after = handle
            .query("SELECT id FROM person", HashMap::new())
            .await
            .expect("query matview");
        assert_eq!(
            matview_after.len(),
            2,
            "a quoted write must maintain the matview like the bare spelling"
        );

        handle.shutdown().await.expect("shutdown");
    }

    // Every write FORM, spelled with a quoted table identifier, must leave the
    // matview in step with the base table. INSERT alone would not cover it:
    // UPDATE and DELETE reach the IVM through their own emit sites.
    #[tokio::test]
    async fn every_quoted_write_form_maintains_the_matview() {
        use std::collections::HashMap;

        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let td = person_td();
        TursoAdapter::register(&td, &handle)
            .await
            .expect("register");

        let rows_in = |handle: &crate::turso::DbHandle, sql: &'static str| {
            let handle = handle.clone();
            async move {
                handle
                    .query(sql, HashMap::new())
                    .await
                    .expect("query")
                    .len()
            }
        };

        for (i, sql) in [
            "INSERT INTO \"person_raw\" (id, email, role) VALUES ('q-0', 'a', 'x')",
            "INSERT OR REPLACE INTO \"person_raw\" (id, email, role) VALUES ('q-1', 'b', 'y')",
            "INSERT INTO\"person_raw\" (id, email, role) VALUES ('q-2', 'c', 'z')",
        ]
        .into_iter()
        .enumerate()
        {
            handle.execute(sql, vec![]).await.expect(sql);
            assert_eq!(
                rows_in(&handle, "SELECT id FROM person").await,
                i + 1,
                "matview must track the quoted insert: {sql}"
            );
        }

        handle
            .execute(
                "UPDATE \"person_raw\" SET email = 'updated' WHERE id = 'q-0'",
                vec![],
            )
            .await
            .expect("quoted update");
        let updated = handle
            .query("SELECT email FROM person WHERE id = 'q-0'", HashMap::new())
            .await
            .expect("query matview");
        assert_eq!(
            updated.first().and_then(|r| r.get("email")),
            Some(&holon_api::Value::String("updated".into())),
            "matview must carry the quoted UPDATE"
        );

        handle
            .execute("DELETE FROM \"person_raw\" WHERE id = 'q-1'", vec![])
            .await
            .expect("quoted delete");
        assert_eq!(
            rows_in(&handle, "SELECT id FROM person").await,
            2,
            "matview must track the quoted DELETE"
        );

        handle.shutdown().await.expect("shutdown");
    }

    // The batch path is the highest-volume writer — QueryableCache's
    // change-stream submits through `transaction`, not `execute` — so the
    // quoted spelling has to maintain the matview here too, not only on the
    // single-statement path.
    #[tokio::test]
    async fn a_quoted_write_inside_a_transaction_batch_maintains_the_matview() {
        use std::collections::HashMap;

        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let td = person_td();
        TursoAdapter::register(&td, &handle)
            .await
            .expect("register");

        let text = |v: &str| turso::Value::Text(v.to_string());
        handle
            .transaction(vec![
                (
                    "INSERT INTO person_raw (id, email) VALUES (?, ?)".to_string(),
                    vec![text("ok-0"), text("a")],
                ),
                (
                    "INSERT INTO \"person_raw\" (id, email) VALUES (?, ?)".to_string(),
                    vec![text("quoted-1"), text("b")],
                ),
                (
                    "INSERT INTO person_raw (id, email) VALUES (?, ?)".to_string(),
                    vec![text("ok-2"), text("c")],
                ),
            ])
            .await
            .expect("a batch mixing quoted and bare spellings must commit");

        let rows = handle
            .query("SELECT id FROM person_raw", HashMap::new())
            .await
            .expect("query raw");
        assert_eq!(rows.len(), 3, "all three statements must have landed");

        let via_matview = handle
            .query("SELECT id FROM person", HashMap::new())
            .await
            .expect("query matview");
        assert_eq!(
            via_matview.len(),
            3,
            "the matview must carry the quoted statement too, not only the bare ones"
        );

        handle.shutdown().await.expect("shutdown");
    }

    // A keyword-named type is rejected at DECLARATION, not at the first write.
    // Measured: without this, `CREATE MATERIALIZED VIEW order AS ...` dies with
    // `near "order": syntax error`, the read matview is never created, and
    // writes to `order_raw` still succeed — a half-created type.
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
            msg.contains("interpolates the name unquoted"),
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
        assert!(
            created.is_empty(),
            "a rejected type must leave no artifacts"
        );

        // A keyword FIELD name is rejected the same way.
        let mut td2 = person_td();
        td2.fields
            .push(FieldSchema::new("select", "TEXT").nullable());
        let err2 = TursoAdapter::register(&td2, &handle)
            .await
            .expect_err("a SQL-keyword field name must be rejected");
        assert!(err2.to_string().contains("select"));

        handle.shutdown().await.expect("shutdown");
    }

    /// The ENGINE property, isolated from the naming policy: a type is declared
    /// (and stored) lowercase, but SQL that SPELLS its names in any case
    /// resolves to the same table and the same matview, and maintenance
    /// follows. This is what the quoted/mixed-case identifier work in the
    /// pinned fork bought; [`TursoAdapter::reject_non_lowercase_name`]
    /// refusing mixed-case type NAMES is a separate concern and does not
    /// weaken it.
    #[tokio::test]
    async fn mixed_case_sql_resolves_to_the_lowercase_type() {
        use std::collections::HashMap;

        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        TursoAdapter::register(&person_td(), &handle)
            .await
            .expect("register the lowercase type");

        // Writes spelled in three different cases, one of them quoted.
        for (id, sql) in [
            (
                "p-0",
                "INSERT INTO PERSON_RAW (id, email, role) VALUES (?, ?, ?)",
            ),
            (
                "p-1",
                "INSERT INTO Person_Raw (id, email, role) VALUES (?, ?, ?)",
            ),
            (
                "p-2",
                "INSERT INTO \"PeRsOn_RaW\" (id, email, role) VALUES (?, ?, ?)",
            ),
        ] {
            handle
                .execute_values(
                    sql,
                    vec![
                        holon_api::Value::String(id.into()),
                        holon_api::Value::String("e".into()),
                        holon_api::Value::String("r".into()),
                    ],
                )
                .await
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
        }

        // Reads spelled in a different case than the stored matview name.
        for sql in [
            "SELECT id FROM person",
            "SELECT id FROM Person",
            "SELECT id FROM PERSON",
        ] {
            let rows = handle
                .query(sql, HashMap::new())
                .await
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
            assert_eq!(
                rows.len(),
                3,
                "every spelling must reach the same maintained matview: {sql}"
            );
        }

        handle.shutdown().await.expect("shutdown");
    }

    /// A mixed-case type NAME is refused at declaration: storage canonicalizes
    /// it away, so two types differing only by case would collide invisibly.
    #[tokio::test]
    async fn a_mixed_case_type_is_rejected_at_registration() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let mut td = person_td();
        td.name = "Person".to_string();

        let msg = TursoAdapter::register(&td, &handle)
            .await
            .expect_err("a mixed-case type name must be rejected")
            .to_string();
        assert!(
            msg.contains("canonicalizes names to lowercase"),
            "the error must name the mechanism; got: {msg}"
        );
        assert!(
            msg.contains("Person") && msg.contains("person"),
            "the error must name the offending identifier AND the fix; got: {msg}"
        );

        // Refused BEFORE any DDL — a rejected type leaves no artifacts, under
        // either spelling (storage would have lowercased it).
        let created = handle
            .query(
                "SELECT name FROM sqlite_master WHERE name LIKE 'person%' COLLATE NOCASE",
                std::collections::HashMap::new(),
            )
            .await
            .expect("query sqlite_master");
        assert!(
            created.is_empty(),
            "a rejected type must leave no artifacts; found {created:?}"
        );

        // An already-lowercase name is untouched by this rule.
        TursoAdapter::register(&person_td(), &handle)
            .await
            .expect("a lowercase type registers normally");

        handle.shutdown().await.expect("shutdown");
    }

    /// The shape rule, on the three ways a lowercase name can still be
    /// unspellable as a bare SQL identifier. Each would otherwise surface as a
    /// DDL syntax error from deep inside `ensure_schema`, naming a generated
    /// statement rather than the type that caused it.
    #[tokio::test]
    async fn a_name_that_is_not_a_bare_identifier_is_rejected_at_registration() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");

        // Hyphen, leading digit, embedded space. All are lowercase, so they
        // clear the case rule and reach this one.
        for bad in ["gen-1", "1x", "a b"] {
            let mut td = person_td();
            td.name = bad.to_string();

            let msg = TursoAdapter::register(&td, &handle)
                .await
                .expect_err("a non-identifier type name must be rejected")
                .to_string();
            assert!(
                msg.contains("cannot be serialized to Turso") && msg.contains("[a-z][a-z0-9_]*"),
                "the error must name the rule; got: {msg}"
            );
            assert!(
                msg.contains(bad),
                "the error must name the offending identifier; got: {msg}"
            );
            assert!(
                msg.contains("may keep any name the registry accepts"),
                "the error must scope itself to the Turso serialization — the registry carries \
                 hyphenated types on purpose; got: {msg}"
            );
        }

        // Refused BEFORE any DDL.
        let created = handle
            .query(
                "SELECT name FROM sqlite_master WHERE name LIKE 'gen-1%' OR name LIKE '1x%'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("query sqlite_master");
        assert!(
            created.is_empty(),
            "a rejected type must leave no artifacts"
        );

        // Underscores and digits AFTER the first character are legal — this is
        // exactly the `gen_N` shape the keystone generator draws.
        let mut ok = person_td();
        ok.name = "gen_1".to_string();
        TursoAdapter::register(&ok, &handle)
            .await
            .expect("a bare identifier registers normally");

        handle.shutdown().await.expect("shutdown");
    }

    // A quoted write reaching the adapter's own matview keeps it in step. The
    // engine resolves the written table through the schema before keying the
    // IVM delta, so the quoted spelling names the same circuit input the bare
    // one does.
    #[tokio::test]
    async fn a_quoted_write_is_maintained_through_the_matview() {
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
                "INSERT INTO \"person_raw\" (id, email, role) VALUES (?, ?, ?)",
                vec![
                    holon_api::Value::String("person-0".into()),
                    holon_api::Value::String("bcdpa".into()),
                    holon_api::Value::String("a".into()),
                ],
            )
            .await
            .expect("a double-quoted write target must be accepted");

        let via_matview = handle
            .query("SELECT id, email, role FROM person", HashMap::new())
            .await
            .expect("query matview");
        assert_eq!(
            via_matview.len(),
            1,
            "the quoted write must be visible through the matview"
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
