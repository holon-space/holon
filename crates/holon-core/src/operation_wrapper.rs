//! OperationWrapper - Decorator for automatic change propagation after
//! operations
//!
//! This module provides a wrapper around OperationProvider that handles:
//! - Sync to external systems via SyncableProvider after operation execution
//! - Future: Cache updates via FieldDelta propagation

use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::StreamPosition;

use crate::storage::types::StorageEntity;
use crate::traits::OperationProvider;
use crate::traits::OperationResult;
use crate::traits::Result;
use crate::traits::SyncableProvider;
use crate::traits::generate_sync_operation;

/// Wrapper that adds automatic sync after operation execution.
///
/// This decorator wraps an OperationProvider and automatically calls
/// sync_changes() on the SyncableProvider after each operation completes.
pub struct OperationWrapper<S> {
    inner: Arc<dyn OperationProvider>,
    sync_provider: Option<Arc<S>>,
}

impl<S> OperationWrapper<S> {
    /// Create a new OperationWrapper with an inner provider and optional sync
    /// provider.
    ///
    /// `inner` is a `dyn OperationProvider` so the wrapper is agnostic to which
    /// backend owns the wrapped CRUD authority (Loro, SQL, …) — the caller
    /// picks the authority and erases its concrete type at the boundary.
    pub fn new(inner: Arc<dyn OperationProvider>, sync_provider: Option<Arc<S>>) -> Self {
        Self {
            inner,
            sync_provider,
        }
    }

    /// Create a wrapper without sync (passthrough mode)
    pub fn without_sync(inner: Arc<dyn OperationProvider>) -> Self {
        Self {
            inner,
            sync_provider: None,
        }
    }
}

#[async_trait]
impl<S> OperationProvider for OperationWrapper<S>
where
    S: SyncableProvider + Send + Sync,
{
    fn operations(&self) -> Vec<OperationDescriptor> {
        let mut ops = self.inner.operations();

        // Add sync operation if sync_provider is present
        if let Some(ref sync_provider) = self.sync_provider {
            ops.push(generate_sync_operation(sync_provider.provider_name()));
        }

        ops
    }

    fn find_operations(
        &self,
        entity_name: &EntityName,
        available_args: &[String],
    ) -> Vec<OperationDescriptor> {
        self.inner.find_operations(entity_name, available_args)
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        // Handle sync operation specially - delegates to sync_provider
        if op_name == "sync" {
            if let Some(ref sync_provider) = self.sync_provider {
                tracing::info!(
                    "[OperationWrapper] Executing sync operation for provider '{}'",
                    sync_provider.provider_name()
                );
                sync_provider.sync(StreamPosition::Beginning).await?;
                return Ok(OperationResult::irreversible(Vec::new()));
            } else {
                return Err("No sync provider configured".into());
            }
        }

        // 1. Execute operation on inner provider
        let result = self
            .inner
            .execute_operation(entity_name, op_name, params)
            .await?;

        // 2. Sync to external systems (if sync provider is available)
        // Extract FieldDeltas from the operation result and pass to sync_changes
        if let Some(ref sync_provider) = self.sync_provider {
            if let Err(e) = sync_provider.sync_changes(&result.changes).await {
                tracing::warn!(
                    "[OperationWrapper] Post-operation sync failed for {}.{}: {}",
                    entity_name,
                    op_name,
                    e
                );
            }
        }

        // 3. Return operation result (contains both changes and undo action)
        Ok(result)
    }

    fn get_last_created_id(&self) -> Option<String> {
        self.inner.get_last_created_id()
    }

    fn identity_minter(&self) -> Option<&dyn holon_api::identity_minting::IdentityMinting> {
        self.inner.identity_minter()
    }

    /// Forwarded like every other non-sync method: this decorator only takes a
    /// position on post-operation sync. Taking the trait's `None` default
    /// would answer "cannot read marks" for a provider that can — and the
    /// wrapper is the registered set member in both wiring arms, so the
    /// dispatcher's fail-safe would silently blind ground-truth mark
    /// comparison to the CRUD authority underneath.
    async fn read_block_content_marks(
        &self,
        id: &str,
    ) -> Result<Option<(String, holon_api::Value)>> {
        self.inner.read_block_content_marks(id).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::traits::FieldDelta;
    use crate::traits::SyncableProvider;

    // Mock OperationProvider for testing
    struct MockProvider;

    #[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
    #[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
    impl OperationProvider for MockProvider {
        fn operations(&self) -> Vec<OperationDescriptor> {
            vec![]
        }

        fn find_operations(&self, _: &EntityName, _: &[String]) -> Vec<OperationDescriptor> {
            vec![]
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

    // Mock SyncableProvider for testing
    struct MockSyncProvider;

    #[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
    #[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
    impl SyncableProvider for MockSyncProvider {
        fn provider_name(&self) -> &str {
            "mock"
        }

        async fn sync(&self, _: StreamPosition) -> Result<StreamPosition> {
            Ok(StreamPosition::Beginning)
        }

        async fn sync_changes(&self, _: &[FieldDelta]) -> Result<()> {
            Ok(())
        }
    }

    /// An inner provider that answers the two accessors the wrapper must not
    /// shadow — enough to tell a forward from the trait's `None` default.
    struct ReadableAuthority;

    #[async_trait]
    impl holon_api::identity_minting::IdentityMinting for ReadableAuthority {
        async fn mint(
            &self,
            _: holon_api::identity_minting::IdentityInput,
        ) -> std::result::Result<
            holon_api::identity_minting::MintedId,
            holon_api::identity_minting::BoxError,
        > {
            Ok(holon_api::identity_minting::MintedId::random())
        }
    }

    #[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
    #[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
    impl OperationProvider for ReadableAuthority {
        fn operations(&self) -> Vec<OperationDescriptor> {
            vec![]
        }

        async fn execute_operation(
            &self,
            _: &EntityName,
            _: &str,
            _: StorageEntity,
        ) -> Result<OperationResult> {
            unreachable!("this fixture only answers accessors")
        }

        fn identity_minter(&self) -> Option<&dyn holon_api::identity_minting::IdentityMinting> {
            Some(self)
        }

        async fn read_block_content_marks(
            &self,
            id: &str,
        ) -> Result<Option<(String, holon_api::Value)>> {
            Ok(Some((
                format!("content of {id}"),
                holon_api::Value::String("[marks]".to_string()),
            )))
        }
    }

    /// The wrapper is the registered set member in BOTH wiring arms, so a
    /// method it does not forward answers the trait's `None` for a provider
    /// that CAN read — and the dispatcher's fail-safe then silently drops the
    /// ground-truth marks comparison (task #23).
    #[tokio::test]
    async fn read_block_content_marks_reaches_the_inner_provider() {
        let provider: Arc<dyn OperationProvider> = Arc::new(ReadableAuthority);
        let wrapper: OperationWrapper<MockSyncProvider> = OperationWrapper::without_sync(provider);

        assert_eq!(
            wrapper
                .read_block_content_marks("block:x")
                .await
                .expect("forwarded read"),
            Some((
                "content of block:x".to_string(),
                holon_api::Value::String("[marks]".to_string())
            )),
            "the wrapper must forward reads to the provider it wraps, not answer None"
        );
    }

    /// Same class as the marks accessor: answering `None` here would tell a
    /// caller the wrapped authority holds no mint executor when it does.
    #[tokio::test]
    async fn identity_minter_reaches_the_inner_provider() {
        let provider: Arc<dyn OperationProvider> = Arc::new(ReadableAuthority);
        let wrapper: OperationWrapper<MockSyncProvider> = OperationWrapper::without_sync(provider);

        assert!(
            wrapper.identity_minter().is_some(),
            "the wrapper must surface the inner provider's mint executor"
        );
    }

    #[tokio::test]
    async fn test_wrapper_passthrough() {
        let provider: Arc<dyn OperationProvider> = Arc::new(MockProvider);
        let wrapper: OperationWrapper<MockSyncProvider> = OperationWrapper::without_sync(provider);

        let result = wrapper
            .execute_operation(&EntityName::from("test"), "test_op", HashMap::new())
            .await;

        assert!(result.is_ok());
    }
}
