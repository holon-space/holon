//! `OperationSubset` — advertise only a whitelisted slice of an inner
//! provider's operations.
//!
//! # Why
//!
//! The operation registry routes a dispatch to the FIRST registered provider
//! whose `operations()` advertises the `(entity, op)` pair (first-registered
//! wins, see `operation_dispatcher.rs`). A provider registered LAST purely to
//! serve a handful of ops that no earlier provider offers should advertise ONLY
//! those ops — otherwise every op it also happens to implement shows up a
//! second time in the aggregated menu (`OperationDispatcher::operations()`
//! unions without dedup), producing duplicate slash-menu entries (BugFunnel
//! N1).
//!
//! Concretely: under Loro authority the block-CRUD provider is
//! `LoroBlockOperations`, which does not advertise the SQL-side link/page
//! transform ops (`create_page_from_link`, `rewrite_link_resolution`,
//! `restore_link_resolution`, `block_to_page_plan`). A bare
//! `SqlOperationProvider` is registered last to serve exactly those — but as a
//! full provider it ALSO advertises `create`/`set_field`/`delete`/… which the
//! Loro provider already owns, duplicating them. Wrapping it in an
//! `OperationSubset` restricted to the four link/page ops keeps the registry
//! duplicate-free while preserving `convert_block_to_page` under Loro.
//!
//! `execute_operation` delegates unconditionally — the dispatcher only routes
//! an op here when this wrapper advertised it, so the inner provider is always
//! the right handler.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::OperationDescriptor;

use crate::storage::types::StorageEntity;
use crate::traits::OperationProvider;
use crate::traits::OperationResult;
use crate::traits::Result;

/// Decorator restricting an inner provider's advertised operations to an
/// allowlist of op names. An empty allowlist makes the wrapper fully inert
/// (advertises nothing, so the dispatcher never routes to it).
pub struct OperationSubset {
    inner: Arc<dyn OperationProvider>,
    allowed: HashSet<String>,
}

impl OperationSubset {
    /// Wrap `inner`, advertising only ops whose name is in `allowed`.
    pub fn new<I>(inner: Arc<dyn OperationProvider>, allowed: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        Self {
            inner,
            allowed: allowed.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
impl OperationProvider for OperationSubset {
    fn operations(&self) -> Vec<OperationDescriptor> {
        self.inner
            .operations()
            .into_iter()
            .filter(|op| self.allowed.contains(&op.name))
            .collect()
    }

    fn find_operations(
        &self,
        entity_name: &EntityName,
        available_args: &[String],
    ) -> Vec<OperationDescriptor> {
        self.inner
            .find_operations(entity_name, available_args)
            .into_iter()
            .filter(|op| self.allowed.contains(&op.name))
            .collect()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        self.inner
            .execute_operation(entity_name, op_name, params)
            .await
    }

    fn get_last_created_id(&self) -> Option<String> {
        self.inner.get_last_created_id()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    struct FullProvider;

    #[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
    #[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
    impl OperationProvider for FullProvider {
        fn operations(&self) -> Vec<OperationDescriptor> {
            ["create", "delete", "block_to_page_plan"]
                .into_iter()
                .map(|name| OperationDescriptor {
                    entity_name: "block".into(),
                    entity_short_name: "block".to_string(),
                    id_column: "id".to_string(),
                    name: name.to_string(),
                    display_name: name.to_string(),
                    description: String::new(),
                    required_params: vec![],
                    affected_fields: vec![],
                    param_mappings: vec![],
                    target_scope: holon_api::TargetScope::Block,
                    boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                    menu_exposure: holon_api::MenuExposure::NotListed {
                        surface: holon_api::NonMenuSurface::Internal,
                    },
                    trigger: None,
                    bound_params: Default::default(),
                    precondition: None,
                })
                .collect()
        }

        fn find_operations(&self, _: &EntityName, _: &[String]) -> Vec<OperationDescriptor> {
            self.operations()
        }

        async fn execute_operation(
            &self,
            _: &EntityName,
            _: &str,
            _: StorageEntity,
        ) -> Result<OperationResult> {
            Ok(OperationResult::irreversible(Vec::new()))
        }
    }

    #[test]
    fn advertises_only_allowlisted_ops() {
        let subset = OperationSubset::new(
            Arc::new(FullProvider) as Arc<dyn OperationProvider>,
            ["block_to_page_plan"],
        );
        let names: Vec<String> = subset.operations().into_iter().map(|o| o.name).collect();
        assert_eq!(names, vec!["block_to_page_plan".to_string()]);
    }

    #[test]
    fn empty_allowlist_is_inert() {
        let subset = OperationSubset::new(
            Arc::new(FullProvider) as Arc<dyn OperationProvider>,
            Vec::<String>::new(),
        );
        assert!(subset.operations().is_empty());
        assert!(
            subset
                .find_operations(&EntityName::from("block"), &[])
                .is_empty()
        );
    }

    #[tokio::test]
    async fn execute_delegates_to_inner() {
        let subset = OperationSubset::new(
            Arc::new(FullProvider) as Arc<dyn OperationProvider>,
            ["block_to_page_plan"],
        );
        let r = subset
            .execute_operation(
                &EntityName::from("block"),
                "block_to_page_plan",
                HashMap::new(),
            )
            .await;
        assert!(r.is_ok());
    }
}
