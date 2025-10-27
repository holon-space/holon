//! Provider trait for obtaining `MutableText` handles keyed by (block_id, field).
//!
//! `LoroEditableTextProvider` caches handles so re-renders don't churn
//! subscriptions. Assumes `seed_loro_from_persistent_store` already
//! populated every block's `content_raw` LoroText at startup — an empty
//! container at editor-open time is a hard error.

use anyhow::{anyhow, Result};
use holon::sync::mutable_text::MutableText;
use loro::LoroText;
use std::sync::Arc;

/// Resolve a stable block ID to a `LoroText` container within a LoroDoc.
pub trait EditableTextProvider: Send + Sync {
    /// Get or create a `MutableText` handle for the given block and field.
    fn editable_text(&self, block_id: &str, field: &str) -> Result<MutableText>;
}

// ── Loro-backed implementation ──────────────────────────────────────────

/// Resolves block_id → LoroText container.
pub trait TextContainerResolver: Send + Sync {
    fn resolve_container(&self, block_id: &str, field: &str) -> Result<LoroText>;
    fn doc(&self) -> Arc<loro::LoroDoc>;
}

/// Production `EditableTextProvider` backed by a shared `LoroDoc`.
pub struct LoroEditableTextProvider {
    resolver: Arc<dyn TextContainerResolver>,
    cache: std::sync::Mutex<std::collections::HashMap<(String, String), MutableText>>,
}

impl LoroEditableTextProvider {
    pub fn new(resolver: Arc<dyn TextContainerResolver>) -> Self {
        Self {
            resolver,
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl EditableTextProvider for LoroEditableTextProvider {
    fn editable_text(&self, block_id: &str, field: &str) -> Result<MutableText> {
        let key = (block_id.to_string(), field.to_string());
        {
            let cache = self.cache.lock().unwrap();
            if let Some(mt) = cache.get(&key) {
                return Ok(mt.clone());
            }
        }
        let text = self.resolver.resolve_container(block_id, field)?;
        let mt = MutableText::new(self.resolver.doc(), text)?;
        let mut cache = self.cache.lock().unwrap();
        cache.entry(key).or_insert_with(|| mt.clone());
        Ok(mt)
    }
}

// ── Reusable resolver: LoroDoc → LoroText by stable block ID ───────────

/// Resolves `block_id` → `LoroText` container by scanning the Loro tree
/// for a node whose `stable_id` metadata matches.
pub struct LoroDocTextResolver {
    pub doc: Arc<loro::LoroDoc>,
}

impl TextContainerResolver for LoroDocTextResolver {
    fn resolve_container(&self, block_id: &str, field: &str) -> Result<LoroText> {
        let tree = self.doc.get_tree(holon::api::loro_backend::TREE_NAME);
        for node in tree.get_nodes(false) {
            if matches!(
                node.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            ) {
                continue;
            }
            let meta = tree.get_meta(node.id)?;
            if let Some(loro::ValueOrContainer::Value(v)) =
                meta.get(holon::api::loro_backend::STABLE_ID)
            {
                if v.as_string()
                    .map(|s| s.to_string() == block_id)
                    .unwrap_or(false)
                {
                    return meta
                        .get_or_create_container(field, loro::LoroText::new())
                        .map_err(|e| anyhow::anyhow!("get_or_create_container: {:?}", e));
                }
            }
        }
        Err(anyhow!("Block {block_id} not found in Loro tree"))
    }

    fn doc(&self) -> Arc<loro::LoroDoc> {
        self.doc.clone()
    }
}

// ── Default (Err) implementation for headless/test services ─────────────

pub struct NoopEditableTextProvider;

impl EditableTextProvider for NoopEditableTextProvider {
    fn editable_text(&self, _block_id: &str, _field: &str) -> Result<MutableText> {
        Err(anyhow!("EditableTextProvider not configured (noop)"))
    }
}
