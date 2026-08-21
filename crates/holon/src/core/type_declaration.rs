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
    let provider: Arc<dyn OperationProvider> =
        Arc::new(SqlOperationProvider::for_type(db_handle.clone(), type_def));
    dispatcher.register_provider(provider)?;

    // A type whose serialization exists but whose writes would be dropped is
    // exactly the trap this check exists for — caught HERE, not at the first
    // write.
    dispatcher
        .assert_write_capability_for(&type_def.name)
        .map_err(|e| format!("'{}' is serialized but not writable: {e}", type_def.name))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use holon_api::FieldSchema;

    use super::*;
    use crate::storage::turso::TursoBackend;

    fn declared(name: &str) -> TypeDefinition {
        TypeDefinition::new(
            name,
            vec![
                FieldSchema::new("id", "TEXT").primary_key(),
                FieldSchema::new("label", "TEXT").nullable(),
            ],
        )
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
