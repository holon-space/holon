//! Declaring a datatype at runtime: the ONE seam that makes a
//! [`TypeDefinition`] real across every layer that must know about it.
//!
//! The datatype is primary; Turso, Loro and org are serializations of it. A
//! declaration therefore does three things and fails loudly if any of them
//! cannot be done:
//!
//! 1. the type enters the [`TypeRegistry`], so lookups by name resolve;
//! 2. its Turso serialization is derived by [`TursoAdapter`] (raw table + read
//!    matview);
//! 3. it gets a WRITE AUTHORITY — a [`SqlOperationProvider`] whose table and
//!    column vocabulary come from the type's own definition — registered on the
//!    dispatcher, so writes route by ITS type instead of finding no provider
//!    (or, worse, block's).
//!
//! Nothing here branches per TYPE. Every per-type artifact is an output of
//! `adapter(TypeDefinition)`, which is what makes declaring a new type a
//! data change rather than a code change.
//!
//! DECLARATION IS ONE-WAY IN THIS INCREMENT. A name, once declared, stays
//! declared for the life of the dispatcher: the declared-authority registry is
//! append-only and [`TursoAdapter::teardown`] drops only the SQL artifacts, so
//! `declare → teardown → declare` does NOT round-trip. Re-declaring therefore
//! fails at step 3 and cannot be recovered from. Undeclaring is the migrate
//! primitive's job (OQ-5) and arrives with it.

use std::sync::Arc;

use holon_api::ColumnValueKind;
use holon_api::TypeDefinition;
use holon_core::OperationProvider;
use holon_core::Result;
use holon_profiles::TypeRegistry;
use holon_turso::turso_adapter::TursoAdapter;
use holon_turso::turso_adapter::TursoArtifacts;

use crate::api::operation_dispatcher::OperationDispatcher;
use crate::core::sql_operation_provider::SqlOperationProvider;
use crate::storage::turso::DbHandle;

/// Declare `type_def` and make it writable. Returns the Turso artifacts the
/// declaration created, so a caller can drop that type's SQL surface again.
///
/// Dropping those artifacts does NOT undeclare the type — see the module doc.
/// Declaring the same name twice fails and stays failed.
pub async fn declare_type(
    type_def: &TypeDefinition,
    db_handle: &DbHandle,
    registry: &TypeRegistry,
    dispatcher: &OperationDispatcher,
) -> Result<TursoArtifacts> {
    // Serialization FIRST: the adapter is where a name the engine cannot
    // safely carry (SQL keyword, mixed case, non-identifier shape) is refused,
    // and refusing it here leaves the registry untouched rather than holding a
    // type nothing can write. That guarantee covers THIS step only — a failure
    // at step 3 refuses the declaration with the registry already mutated,
    // which is unrecoverable for that name (see the module doc).
    let artifacts = TursoAdapter::register(type_def, db_handle)
        .await
        .map_err(|e| {
            format!(
                "declare_type('{}'): deriving the Turso serialization failed: {e}",
                type_def.name
            )
        })?;

    registry.register(type_def.clone()).map_err(|e| {
        format!(
            "declare_type('{}'): the type registry refused the definition: {e}",
            type_def.name
        )
    })?;

    register_write_authority(type_def, db_handle, dispatcher).map_err(|e| {
        format!(
            "declare_type('{}'): registering the write authority failed: {e}",
            type_def.name
        )
    })?;

    Ok(artifacts)
}

/// Give an already-serialized type its write authority.
///
/// Split out because the two ways a type becomes real share this step: a type
/// seeded in the registry at boot already has its Turso artifacts (derived by
/// `FreeStandingTypeViews`) and needs only the authority, while
/// [`declare_type`] does the whole sequence. One derivation either way —
/// `SqlOperationProvider::for_type` reads the definition and nothing else.
pub fn register_write_authority(
    type_def: &TypeDefinition,
    db_handle: &DbHandle,
    dispatcher: &OperationDispatcher,
) -> Result<()> {
    require_engine_stamp_has_a_home(type_def)?;
    require_declarable_soft_delete(type_def)?;

    let provider: Arc<dyn OperationProvider> =
        Arc::new(SqlOperationProvider::for_type(db_handle.clone(), type_def));
    dispatcher.register_provider(provider)?;

    dispatcher
        .assert_write_capability_for(&type_def.name)
        .map_err(|e| format!("'{}' is serialized but not writable: {e}", type_def.name))?;
    Ok(())
}

/// Every type must declare the overflow column the engine's `_provenance`
/// stamp lands in; the write boundary routes no field it has no column for.
fn require_engine_stamp_has_a_home(type_def: &TypeDefinition) -> Result<()> {
    let overflow = crate::core::sql_operation_provider::WriteSchema::OVERFLOW_COLUMN;
    let declared = type_def
        .persistent_fields()
        .into_iter()
        .find(|f| f.name == overflow);
    match declared {
        Some(field) if field.value_kind == ColumnValueKind::OverflowProperties => Ok(()),
        // A column named `properties` that does not declare itself the overflow
        // bag is an ordinary column to everything that reads the declaration —
        // the keystone's datatype axis would fill it with a drawn value, which
        // the engine's stamp then has to share.
        Some(field) => Err(format!(
            "type '{name}': `{overflow}` is declared `value_kind: {kind:?}`, but the engine's \
             `_provenance` stamp lands in it, so it must declare \
             `value_kind: overflow_properties`",
            name = type_def.name,
            kind = field.value_kind,
        )
        .into()),
        None => Err(format!(
            "type '{name}' declares no `{overflow}` overflow column, so the engine's `_provenance` \
             stamp would have nowhere to land and EVERY `create` and `update` of it would be \
             refused at the write boundary. Add the pair `{overflow}` \
             (`value_kind: overflow_properties`) and `property_kinds` \
             (`value_kind: overflow_property_kinds`) to the type definition. Declared persisted \
             fields: {fields:?}",
            name = type_def.name,
            fields = type_def
                .persistent_fields()
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
        )
        .into()),
    }
}

/// A soft-delete declaration must name a persisted field of the type, nullable
/// TEXT (an RFC 3339 stamp, absent on a live row), with a positive retention.
fn require_declarable_soft_delete(type_def: &TypeDefinition) -> Result<()> {
    let Some(soft_delete) = &type_def.soft_delete else {
        return Ok(());
    };
    let name = &type_def.name;
    let field = type_def
        .persistent_fields()
        .into_iter()
        .find(|f| f.name == soft_delete.tombstone_field)
        .ok_or_else(|| {
            format!(
                "type '{name}' declares soft deletion into '{}', which is not one of its \
                 persisted fields — `delete` would write a column the table does not have",
                soft_delete.tombstone_field
            )
        })?;
    if !field.nullable || !field.sql_type.eq_ignore_ascii_case("TEXT") {
        return Err(format!(
            "type '{name}': the tombstone field '{}' is declared `{} {}`, but a tombstone holds \
             an RFC 3339 timestamp and is absent on a live row — declare it nullable TEXT",
            field.name,
            field.sql_type,
            if field.nullable {
                "nullable"
            } else {
                "not null"
            },
        )
        .into());
    }
    if soft_delete.retention_days <= 0 {
        return Err(format!(
            "type '{name}': a soft-delete retention of {} day(s) expires every tombstone as soon \
             as it is written, which is a hard delete with extra steps",
            soft_delete.retention_days
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use holon_api::FieldSchema;

    use super::*;
    use crate::storage::turso::TursoBackend;

    /// A minimal declarable type, carrying the overflow pair every type needs.
    fn declared(name: &str) -> TypeDefinition {
        let mut fields = vec![
            FieldSchema::new("id", "TEXT").primary_key(),
            FieldSchema::new("label", "TEXT").nullable(),
        ];
        fields.extend(FieldSchema::overflow_pair());
        TypeDefinition::new(name, fields)
    }

    /// Entry `docs/Testing/bugfunnel/entries/
    /// 2026-09-02-a-shopping-item-can-never-be-added-in-holon.md`: the refusal
    /// reaches the yaml author at declaration time.
    #[test]
    fn a_type_that_cannot_hold_the_engine_stamp_is_refused_at_declaration() {
        let type_def = TypeDefinition::new(
            "gen_no_overflow",
            vec![FieldSchema::new("id", "TEXT").primary_key()],
        );
        let err = require_engine_stamp_has_a_home(&type_def)
            .expect_err("a type with nowhere for `_provenance` must be refused")
            .to_string();
        assert!(
            err.contains("gen_no_overflow") && err.contains("properties"),
            "the refusal must name the type and the missing column; got: {err}"
        );
        require_engine_stamp_has_a_home(&declared("gen_ok"))
            .expect("the overflow pair is all this check asks for");
    }

    fn with_soft_delete(field: &str, retention_days: i64) -> TypeDefinition {
        let mut type_def = declared("gen_soft");
        type_def
            .fields
            .push(FieldSchema::new("gone_at", "TEXT").nullable());
        type_def.fields.push(FieldSchema::new("count", "INTEGER"));
        type_def.soft_delete = Some(holon_api::entity::SoftDelete {
            tombstone_field: field.to_string(),
            retention_days,
        });
        type_def
    }

    /// Each arm is a soft-delete declaration the engine cannot honour: a column
    /// the type does not have, a NOT NULL or non-text one, an expired window.
    #[test]
    fn an_unhonourable_soft_delete_declaration_is_refused() {
        require_declarable_soft_delete(&with_soft_delete("gone_at", 7))
            .expect("a nullable TEXT tombstone with a real window is declarable");

        for (field, retention, expected) in [
            ("no_such_column", 7, "not one of its persisted fields"),
            ("count", 7, "declare it nullable TEXT"),
            ("gone_at", 0, "expires every tombstone"),
        ] {
            let err = require_declarable_soft_delete(&with_soft_delete(field, retention))
                .expect_err("an unhonourable declaration must be refused")
                .to_string();
            assert!(
                err.contains(expected),
                "expected a refusal mentioning {expected:?}, got: {err}"
            );
        }
    }

    /// Every bundled declaration passes the guard. Read from the bundled
    /// registry, so a newly added yaml is covered without editing this test.
    #[test]
    fn every_bundled_type_with_a_derived_authority_can_hold_the_stamp() {
        let registry = holon_profiles::create_default_registry().expect("the bundled types parse");
        holon_kitchen::register_kitchen_types(&registry).expect("the kitchen types parse");

        let mut swept = 0;
        for type_def in registry.all() {
            if !crate::di::schema_providers::is_free_standing(&type_def)
                || type_def.owning_integration().is_some()
            {
                continue;
            }
            swept += 1;
            require_engine_stamp_has_a_home(&type_def)
                .unwrap_or_else(|e| panic!("bundled type '{}': {e}", type_def.name));
            require_declarable_soft_delete(&type_def)
                .unwrap_or_else(|e| panic!("bundled type '{}': {e}", type_def.name));
        }
        assert!(
            swept >= 4,
            "the sweep found only {swept} bundled free-standing type(s); it is looking in the \
             wrong place"
        );
    }

    /// The two declaration checks are WIRED into the seam every declaration
    /// passes through, not merely present as functions: this drives the public
    /// [`declare_type`] and expects the refusal, and expects the name to stay
    /// unwritable after it.
    #[tokio::test]
    async fn declaring_an_undeclarable_type_is_refused_by_the_public_path() {
        let (_backend, db_handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let registry = TypeRegistry::new();
        let dispatcher = OperationDispatcher::new(vec![]);

        let no_overflow = TypeDefinition::new(
            "gen_no_overflow",
            vec![
                FieldSchema::new("id", "TEXT").primary_key(),
                FieldSchema::new("label", "TEXT").nullable(),
            ],
        );
        // A tombstone holds an RFC 3339 stamp and is absent on a live row, so
        // an INTEGER NOT NULL column cannot be one. The column EXISTS, which is
        // what makes the type serializable and carries it as far as step 3 —
        // a missing column is refused earlier, by the adapter's matview filter.
        let mut unhonourable = declared("gen_unhonourable");
        unhonourable
            .fields
            .push(FieldSchema::new("count", "INTEGER"));
        unhonourable.soft_delete = Some(holon_api::entity::SoftDelete {
            tombstone_field: "count".to_string(),
            retention_days: 7,
        });

        for (type_def, expected) in [
            (&no_overflow, "declares no `properties` overflow column"),
            (&unhonourable, "declare it nullable TEXT"),
        ] {
            let err = declare_type(type_def, &db_handle, &registry, &dispatcher)
                .await
                .expect_err("an undeclarable type must be refused by the public path")
                .to_string();
            assert!(
                err.contains(expected),
                "expected a refusal mentioning {expected:?}, got: {err}"
            );
            assert!(
                !dispatcher.has_provider(&type_def.name),
                "'{}' must not become writable",
                type_def.name
            );
        }
    }

    /// Declaration is ONE-WAY in this increment, and the error says so. This
    /// test exercises the behaviour rather than the wording: it re-declares,
    /// then tears the SQL artifacts down and re-declares AGAIN, and shows the
    /// name is still not free. That second half is the part that would have
    /// caught the old error text, which told the reader to do exactly this.
    ///
    /// When the migrate primitive (OQ-5) lands, this test is where the new
    /// contract gets written — it must be changed deliberately, not deleted.
    #[tokio::test]
    async fn a_declared_type_cannot_be_redeclared_even_after_teardown() {
        let (_backend, db_handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        let registry = TypeRegistry::new();
        let dispatcher = OperationDispatcher::new(vec![]);
        let type_def = declared("gen_1");

        let artifacts = declare_type(&type_def, &db_handle, &registry, &dispatcher)
            .await
            .expect("first declaration");
        assert!(registry.contains("gen_1"), "the type entered the registry");
        assert!(dispatcher.has_provider("gen_1"), "the type became writable");

        // Re-declaring is refused, and refused at the write-authority step.
        let err = declare_type(&type_def, &db_handle, &registry, &dispatcher)
            .await
            .expect_err("re-declaring a live type must be refused")
            .to_string();
        assert!(
            err.contains("registering the write authority failed"),
            "the refusal must come from step 3; got: {err}"
        );
        assert!(
            !err.contains("Tear the type down"),
            "the error must not instruct an action with no code path; got: {err}"
        );

        // Dropping the SQL artifacts does NOT free the name: the authority and
        // the registry entry both survive, so the retry fails identically.
        // This is the whole content of the "one-way" claim.
        TursoAdapter::teardown(&artifacts, &db_handle)
            .await
            .expect("teardown drops the SQL artifacts");
        assert!(
            registry.contains("gen_1"),
            "teardown is a SQL-level operation — it does not undeclare the type"
        );
        assert!(
            dispatcher.has_provider("gen_1"),
            "teardown leaves the write authority in place, which is why the name stays taken"
        );

        let after_teardown = declare_type(&type_def, &db_handle, &registry, &dispatcher)
            .await
            .expect_err("teardown must not make the name re-declarable")
            .to_string();
        assert!(
            after_teardown.contains("registering the write authority failed"),
            "the post-teardown retry must fail the same way; got: {after_teardown}"
        );
    }
}
