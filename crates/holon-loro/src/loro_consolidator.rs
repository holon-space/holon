//! Loro as a [`holon_core::consolidator::Consolidator`]: the history seam, as
//! distinct from [`crate::consolidator::BlockConsolidator`], which owns the
//! SQL sink write.

use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::capability::CapabilityProfile;
use holon_api::capability::ConsolidatorId;
use holon_api::capability::SessionCapabilities;
use holon_core::consolidator::Consolidator;
use holon_core::consolidator::Delta;
use holon_core::consolidator::ReplicaId;
use holon_core::consolidator::Seen;
use holon_core::consolidator::Version;
use holon_core::traits::Result;

use crate::loro_backend::LoroBackend;

pub struct LoroConsolidator {
    backend: Arc<LoroBackend>,
}

impl LoroConsolidator {
    pub fn new(backend: Arc<LoroBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Consolidator for LoroConsolidator {
    async fn is_ancestor(&self, a: &Version, b: &Version) -> Result<bool> {
        Ok(self.backend.is_ancestor(a, b).await?)
    }

    fn epoch_id(&self) -> ConsolidatorId {
        SessionCapabilities::pin(CapabilityProfile::Projected).consolidator_id()
    }

    async fn head(&self) -> Result<Version> {
        Ok(self.backend.head_version().await?)
    }

    /// The Loro→SQL diff is computed by the outbound projector
    /// (`LoroSyncController`), which nothing yet reads through this seam.
    async fn diff(&self, from: &Version, to: &Version) -> Result<Delta> {
        Err(format!(
            "LoroConsolidator::diff({from}, {to}) has no entity-level implementation; the \
             outbound projector owns the Loro→SQL diff"
        )
        .into())
    }

    async fn ever_seen(&self, id: &EntityUri) -> Result<Seen> {
        Ok(self.backend.ever_seen(id).await?)
    }

    /// `LoroDocument::export_compact_snapshot` trims op history on a schedule
    /// and consults no retention claim, so a claim recorded here would promise
    /// history the store does not keep.
    async fn register_base(&self, replica: &ReplicaId, at: &Version) -> Result<()> {
        Err(format!(
            "LoroConsolidator cannot retain history for {} at {at}: the compacting save \
             trims op history unconditionally",
            replica.as_str()
        )
        .into())
    }

    async fn release_base(&self, replica: &ReplicaId) -> Result<()> {
        Err(format!("release_base: {} holds no base", replica.as_str()).into())
    }

    async fn registered_bases(&self) -> Result<Vec<ReplicaId>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use loro::LoroDoc;

    use super::*;
    use crate::loro_document::LoroDocument;
    use crate::multi_peer::create_block_with_id;

    #[tokio::test]
    async fn is_ancestor_orders_heads_along_the_history() {
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1).unwrap();
        let consolidator = LoroConsolidator::new(Arc::new(LoroBackend::from_document(Arc::new(
            LoroDocument::from_existing(doc.clone(), "test"),
        ))));
        create_block_with_id(&doc, None, "a", "a");
        let earlier = consolidator.head().await.unwrap();
        create_block_with_id(&doc, None, "b", "b");
        let later = consolidator.head().await.unwrap();
        assert_ne!(earlier, later, "a commit moves the head");

        assert!(consolidator.is_ancestor(&earlier, &later).await.unwrap());
        assert!(!consolidator.is_ancestor(&later, &earlier).await.unwrap());
        assert!(consolidator.is_ancestor(&later, &later).await.unwrap());

        let unknown = Version::new(hex::encode(
            loro::Frontiers::from_id(loro::ID::new(99, 0)).encode(),
        ));
        assert!(
            consolidator.is_ancestor(&unknown, &later).await.is_err(),
            "a version outside this doc's history is an error"
        );
        assert!(
            consolidator
                .is_ancestor(&Version::new("not-hex"), &later)
                .await
                .is_err(),
            "an undecodable version is an error"
        );
    }
}
