//! Parity certificate for the shared block op-catalog (Write-Path Unification
//! Option A increment 0 / Option D —
//! `docs/Plans/WritePathUnification-Options-2026-07-17.md`).
//!
//! Both block write authorities advertise operation descriptors independently:
//! `SqlOperationProvider` (SqlOnly authority) and `LoroBlockOperations` (Loro
//! authority). The macro-generated CRUD/task/block/mark/text descriptors are
//! already single-sourced from the `#[operations_trait]` macro, and the two
//! providers deliberately advertise different *supersets* (the SQL provider is
//! a minimal generic table writer; the Loro provider advertises the full block
//! vocabulary). The remaining drift surface is the **bespoke** descriptors that
//! are hand-built in both — today exactly `dismiss_advice`, whose prior
//! independent duplication was BugFunnel row 26.
//!
//! This test asserts every descriptor the shared catalog owns appears
//! **byte-identical** in BOTH providers' `operations()`. It is the certificate
//! that no future edit re-forks a catalog-owned descriptor. As increments 1-2
//! move more op metadata into the catalog, this test's coverage grows with it.

use std::collections::BTreeSet;
use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::storage::schema_module::EdgeFieldDescriptor;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_core::OperationProvider;
use holon_core::UnknownOperationError;
use holon_loro::LoroBlockOperations;
use holon_loro::LoroDocumentStore;
use tokio::sync::RwLock;

const ENTITY: &str = "block";
const SHORT: &str = "block";

/// The block edge fields the prod `BlockSchemaModule` registers. The
/// `advice_suppressed` edge is what gates the SQL provider advertising
/// `dismiss_advice`, so it must be present for a faithful parity check.
fn block_edge_fields() -> Vec<EdgeFieldDescriptor> {
    vec![
        EdgeFieldDescriptor {
            entity: ENTITY.to_string(),
            field: holon_api::EdgeField::Requires,
            join_table: "block_requires".to_string(),
            source_col: "block_id".to_string(),
            target_col: "required_id".to_string(),
        },
        EdgeFieldDescriptor {
            entity: ENTITY.to_string(),
            field: holon_api::EdgeField::Tags,
            join_table: "block_tags".to_string(),
            source_col: "block_id".to_string(),
            target_col: "tag".to_string(),
        },
        EdgeFieldDescriptor {
            entity: ENTITY.to_string(),
            field: holon_api::EdgeField::AdviceSuppressed,
            join_table: "advice_suppressed".to_string(),
            source_col: "anchor_id".to_string(),
            target_col: "lesson_id".to_string(),
        },
    ]
}

fn find_op<'a>(ops: &'a [OperationDescriptor], name: &str) -> &'a OperationDescriptor {
    ops.iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("provider does not advertise op '{name}'"))
}

#[tokio::test]
async fn both_block_providers_source_catalog_descriptors_identically() {
    // SqlOnly authority.
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    let sql_provider = SqlOperationProvider::with_edge_fields(
        handle,
        "block_raw".to_string(),
        ENTITY.to_string(),
        SHORT.to_string(),
        block_edge_fields(),
    );

    // Loro authority.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(RwLock::new(LoroDocumentStore::new(
        dir.path().to_path_buf(),
    )));
    let loro_provider = LoroBlockOperations::new(store);

    let sql_ops = sql_provider.operations();
    let loro_ops = loro_provider.operations();

    // Every descriptor the catalog owns must appear byte-identical in BOTH
    // providers. Adding a catalog entry (increments 1-2) extends this list.
    let entity = EntityName::from(ENTITY);
    let catalog: Vec<OperationDescriptor> = vec![
        holon_core::block_op_catalog::dismiss_advice_descriptor(&entity, SHORT),
        holon_core::block_op_catalog::add_tag_descriptor(&entity, SHORT),
        holon_core::block_op_catalog::remove_tag_descriptor(&entity, SHORT),
        holon_core::block_op_catalog::update_descriptor(
            &entity,
            SHORT,
            holon_api::TargetScope::Block,
            holon_core::block_op_catalog::block_update_references(),
        ),
    ];

    for canonical in &catalog {
        let sql_desc = find_op(&sql_ops, &canonical.name);
        let loro_desc = find_op(&loro_ops, &canonical.name);
        assert_eq!(
            sql_desc, canonical,
            "SqlOperationProvider's '{}' descriptor drifted from the shared catalog",
            canonical.name
        );
        assert_eq!(
            loro_desc, canonical,
            "LoroBlockOperations' '{}' descriptor drifted from the shared catalog",
            canonical.name
        );
    }
}

fn reference_names(ops: &[OperationDescriptor], op: &str) -> BTreeSet<String> {
    holon_api::entity_reference_params(ops, ENTITY, op, |_| None)
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .map(|reference| reference.name.to_string())
        .collect()
}

/// A write authority's claims come from its OWN descriptors: an op it executes
/// but does not declare is claimed as the whole relation, and a declaration
/// missing a reference the op carries elsewhere under-claims it. Every block op
/// either authority declares is run against each one with no params; one it
/// does not declare must be refused as unknown.
#[tokio::test]
async fn each_block_authority_declares_every_op_it_executes_with_its_references() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(RwLock::new(LoroDocumentStore::new(
        dir.path().to_path_buf(),
    )));
    let authorities: Vec<(&str, Arc<dyn OperationProvider>)> = vec![
        (
            "SqlOperationProvider",
            Arc::new(SqlOperationProvider::with_edge_fields(
                handle,
                "block_raw".to_string(),
                ENTITY.to_string(),
                SHORT.to_string(),
                block_edge_fields(),
            )),
        ),
        (
            "LoroBlockOperations",
            Arc::new(LoroBlockOperations::new(store)),
        ),
    ];
    let declared: Vec<Vec<OperationDescriptor>> = authorities
        .iter()
        .map(|(_, provider)| provider.operations())
        .collect();
    let all: Vec<OperationDescriptor> = declared.iter().flatten().cloned().collect();
    let op_names: BTreeSet<String> = all
        .iter()
        .filter(|op| op.entity_name == ENTITY)
        .map(|op| op.name.clone())
        .collect();

    let mut wrong = Vec::new();
    for ((name, provider), ops) in authorities.iter().zip(&declared) {
        for op in &op_names {
            if ops.iter().any(|d| d.entity_name == ENTITY && &d.name == op) {
                let missing: Vec<String> = reference_names(&all, op)
                    .difference(&reference_names(ops, op))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    wrong.push(format!("{name} declares {op} without {missing:?}"));
                }
                continue;
            }
            let outcome = provider
                .execute_operation(&EntityName::from(ENTITY), op, Default::default())
                .await;
            match outcome {
                Err(e) if UnknownOperationError::is_unknown(e.as_ref()) => {}
                other => wrong.push(format!(
                    "{name} executes {op} without declaring it: {:?}",
                    other.map(|_| ()).map_err(|e| e.to_string())
                )),
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
