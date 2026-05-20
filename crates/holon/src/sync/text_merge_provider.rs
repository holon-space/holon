//! Text-merge seam (block-sync rework, Phase 2).
//!
//! A block's `content` is mergeable text. When Loro is present that text lives
//! in a shared `LoroText` CRDT container (concurrent edits merge); without Loro
//! it is a plain, transient string (last-writer-wins, no merge). Today the
//! choice is made implicitly inside [`crate::sync::block_cell_registry`].
//!
//! [`TextMergeProvider`] makes that choice an explicit, capability-driven seam:
//! [`CapabilityProfile::Projected`] → a [`LoroTextMergeProvider`] handing out
//! shared `LoroText`; [`CapabilityProfile::Direct`] → a
//! [`TransientTextMergeProvider`] handing out a plain string.
//!
//! # Phase 2 status: wired, not the sole path
//!
//! This is additive. The registry still resolves text the way it always has;
//! the provider is the seam later phases route through (Phase 3 "route text
//! merges through `TextMergeProvider`"). The `LoroTextMergeProvider` delegates
//! container resolution to an injected closure so it does **not** duplicate or
//! diverge from the registry's container logic — both resolve the same
//! `LoroText`, they just reach it through one boundary.
//!
//! [`CapabilityProfile::Projected`]: crate::sync::capability::CapabilityProfile::Projected
//! [`CapabilityProfile::Direct`]: crate::sync::capability::CapabilityProfile::Direct

use std::sync::Arc;

use anyhow::Result;
use loro::LoroText;

use crate::sync::capability::CapabilityProfile;

/// A handle to a block's mergeable content text.
pub enum TextHandle {
    /// Shared CRDT text — concurrent edits merge. Loro present.
    Loro(LoroText),
    /// Plain transient text — last-writer-wins, no merge. SqlOnly.
    Transient(String),
}

impl TextHandle {
    /// The current string value of the handle.
    pub fn to_string_value(&self) -> String {
        match self {
            TextHandle::Loro(t) => t.to_string(),
            TextHandle::Transient(s) => s.clone(),
        }
    }

    /// Whether edits to this handle merge with concurrent edits (CRDT) or
    /// clobber (last-writer-wins).
    pub fn is_mergeable(&self) -> bool {
        matches!(self, TextHandle::Loro(_))
    }
}

/// Hands out a content-text handle for a block, mergeable iff Loro is present.
pub trait TextMergeProvider: Send + Sync {
    /// The shared/transient text handle for `block_id`'s content.
    fn text_handle(&self, block_id: &str) -> Result<TextHandle>;

    /// The capability profile this provider serves (so callers can branch on
    /// merge semantics without a downcast).
    fn profile(&self) -> CapabilityProfile;
}

/// Resolves the `LoroText` container backing a block's content. Injected so the
/// provider reuses the registry's existing container resolution rather than
/// reimplementing it (avoiding a divergent text home).
pub type LoroTextResolver = Arc<dyn Fn(&str) -> Result<LoroText> + Send + Sync>;

/// [`TextMergeProvider`] for [`CapabilityProfile::Projected`]: returns the
/// shared `LoroText` from the Loro doc via the injected resolver.
pub struct LoroTextMergeProvider {
    resolver: LoroTextResolver,
}

impl LoroTextMergeProvider {
    pub fn new(resolver: LoroTextResolver) -> Self {
        Self { resolver }
    }
}

impl TextMergeProvider for LoroTextMergeProvider {
    fn text_handle(&self, block_id: &str) -> Result<TextHandle> {
        Ok(TextHandle::Loro((self.resolver)(block_id)?))
    }

    fn profile(&self) -> CapabilityProfile {
        CapabilityProfile::Projected
    }
}

/// [`TextMergeProvider`] for [`CapabilityProfile::Direct`]: returns a plain
/// transient string. No merge — degraded mode is last-writer-wins.
pub struct TransientTextMergeProvider;

impl TextMergeProvider for TransientTextMergeProvider {
    fn text_handle(&self, _: &str) -> Result<TextHandle> {
        Ok(TextHandle::Transient(String::new()))
    }

    fn profile(&self) -> CapabilityProfile {
        CapabilityProfile::Direct
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::LoroDoc;

    #[test]
    fn transient_provider_is_not_mergeable() {
        let provider = TransientTextMergeProvider;
        assert_eq!(provider.profile(), CapabilityProfile::Direct);
        let handle = provider.text_handle("block:a").unwrap();
        assert!(!handle.is_mergeable());
        assert_eq!(handle.to_string_value(), "");
    }

    #[test]
    fn loro_provider_returns_shared_mergeable_text() {
        // A resolver backed by a real Loro doc: every call for the same id hands
        // back the same shared container, so a write through one handle is
        // visible through the next — that's the "shared" contract.
        let doc = Arc::new(LoroDoc::new());
        let resolver: LoroTextResolver = {
            let doc = doc.clone();
            Arc::new(move |block_id: &str| {
                let map = doc.get_map("text_by_block");
                let text = map.get_or_create_container(block_id, LoroText::new())?;
                Ok(text)
            })
        };
        let provider = LoroTextMergeProvider::new(resolver);
        assert_eq!(provider.profile(), CapabilityProfile::Projected);

        let handle = provider.text_handle("block:a").unwrap();
        assert!(handle.is_mergeable());
        if let TextHandle::Loro(text) = handle {
            text.insert(0, "hello").unwrap();
        } else {
            panic!("expected Loro handle");
        }
        doc.commit();

        // A fresh resolution sees the prior write — same shared container.
        let again = provider.text_handle("block:a").unwrap();
        assert_eq!(again.to_string_value(), "hello");
    }
}
