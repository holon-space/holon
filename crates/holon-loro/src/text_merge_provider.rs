//! Text-merge seam (block-sync rework, Phase 2).
//!
//! A block's `content` is mergeable text. When Loro is present that text lives
//! in a shared `LoroText` CRDT container (concurrent edits merge); without Loro
//! it is a plain, transient string (last-writer-wins, no merge). Today the
//! choice is made implicitly inside [`crate::block_cell_registry`].
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
//! [`CapabilityProfile::Projected`]: crate::capability::CapabilityProfile::Projected
//! [`CapabilityProfile::Direct`]: crate::capability::CapabilityProfile::Direct

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::ensure;
use holon_filesystem::ThreeWayTextMerge;
use loro::ExportMode;
use loro::LoroDoc;
use loro::LoroText;
use unicode_segmentation::UnicodeSegmentation;

use crate::capability::CapabilityProfile;

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

/// [`ThreeWayTextMerge`] for the no-store conflict path (SqlOnly / `Direct`).
///
/// Performs a base-3-way merge of one block's text content via a **transient**
/// `LoroText`: a throwaway `LoroDoc` seeded with `base`, forked into two peers
/// that each apply one edit (`theirs`, `mine`), then merged. This is the merge
/// *function* only — nothing is stored, cached, or committed to any durable
/// doc; every document created here is dropped when the call returns (Model.md:
/// *transient = merge function only*).
///
/// Merge contract:
/// - each side's edit is the minimal grapheme-cluster diff from `base`, so a
///   base char a side kept keeps its identity, and a delete of it by the other
///   side holds;
/// - a char either side inserted survives, in that side's order;
/// - inserts of both sides into the same gap of `base` come out `mine` first,
///   then `theirs`. A replacement's inserted text sits in the gap before the
///   chars it replaces.
///
/// A side whose diff exceeds [`DIFF_BUDGET`] replaces all of `base` instead,
/// with a warning: nothing is lost, but text both sides kept appears twice.
///
/// Wired into `FileSyncController` in Direct mode via
/// `FileSyncController::with_text_merge`. In Full (Loro-the-store) mode this is
/// never invoked — the live CRDT already merges concurrent edits.
pub struct TransientLoroTextMerge;

/// Loro orders concurrent inserts at one position lower peer id first, which
/// puts `mine` before `theirs`.
const MINE_PEER: u64 = 1;
const THEIRS_PEER: u64 = 2;

/// Wall time one side's diff may take. Myers costs O(N·D), so rewriting a
/// long block can take minutes. `similar` ignores deadlines on wasm32.
const DIFF_BUDGET: Duration = Duration::from_millis(200);

impl ThreeWayTextMerge for TransientLoroTextMerge {
    fn merge_text(&self, base: &str, theirs: &str, mine: &str) -> Result<String> {
        merge_within(base, theirs, mine, DIFF_BUDGET)
    }
}

fn merge_within(base: &str, theirs: &str, mine: &str, budget: Duration) -> Result<String> {
    let ancestor = LoroDoc::new();
    ancestor
        .set_peer_id(0)
        .context("transient merge: set ancestor peer id")?;
    ancestor
        .get_text("content")
        .insert(0, base)
        .context("transient merge: seed base text")?;
    ancestor.commit();
    let snapshot = ancestor
        .export(ExportMode::Snapshot)
        .context("transient merge: export base snapshot")?;

    let fork = |peer: u64, side: &str, label: &str| -> Result<LoroDoc> {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer)
            .with_context(|| format!("transient merge: set {label} peer id"))?;
        doc.import(&snapshot)
            .with_context(|| format!("transient merge: import base into {label}"))?;
        apply_minimal_diff(&doc.get_text("content"), base, side, budget, label)
            .with_context(|| format!("transient merge: apply {label}"))?;
        doc.commit();
        Ok(doc)
    };
    let theirs_doc = fork(THEIRS_PEER, theirs, "theirs")?;
    let mine_doc = fork(MINE_PEER, mine, "mine")?;

    let mine_updates = mine_doc
        .export(ExportMode::all_updates())
        .context("transient merge: export mine updates")?;
    theirs_doc
        .import(&mine_updates)
        .context("transient merge: merge mine into theirs")?;
    Ok(theirs_doc.get_text("content").to_string())
}

/// Edit `text` (holding `base`) into `target` by the minimal diff of grapheme
/// clusters, so a cluster's chars are kept, deleted or replaced as one unit.
///
/// `LoroText::update` is not minimal: it may delete a char and insert an equal
/// one, and the other side's delete of the old char then misses the new one.
/// The ops come from the raw Myers hook: `similar::capture_diff_slices` also
/// compacts them, and its ops' positions then disagree with their order when
/// chars repeat. Each hunk
/// inserts before it deletes, so its text sits in the gap before the replaced
/// chars.
fn apply_minimal_diff(
    text: &LoroText,
    base: &str,
    target: &str,
    budget: Duration,
    label: &str,
) -> Result<()> {
    let old: Vec<&str> = base.graphemes(true).collect();
    let new: Vec<&str> = target.graphemes(true).collect();
    #[cfg(not(target_arch = "wasm32"))]
    let deadline = Some(Instant::now() + budget);
    #[cfg(target_arch = "wasm32")]
    let deadline = None::<Instant>;
    let mut capture = similar::algorithms::Capture::new();
    let Ok(()) = similar::algorithms::myers::diff_deadline(
        &mut capture,
        &old,
        0..old.len(),
        &new,
        0..new.len(),
        deadline,
    );
    if deadline.is_some_and(|deadline| Instant::now() > deadline) {
        tracing::warn!(
            "3-way text merge: diffing {label} ({} -> {} chars) took over {budget:?}; \
             {label} replaces the whole base text, so text both sides kept appears twice",
            base.chars().count(),
            target.chars().count(),
        );
        text.insert(0, target)?;
        text.delete(target.chars().count(), base.chars().count())?;
        return Ok(());
    }

    let chars = |clusters: &[&str]| clusters.iter().map(|g| g.chars().count()).sum::<usize>();
    let (mut old_at, mut new_at, mut text_at) = (0, 0, 0);
    let (mut inserted, mut deleted) = (String::new(), 0);
    let flush = |text_at: &mut usize, inserted: &mut String, deleted: &mut usize| -> Result<()> {
        text.insert(*text_at, inserted)?;
        *text_at += inserted.chars().count();
        text.delete(*text_at, *deleted)?;
        inserted.clear();
        *deleted = 0;
        Ok(())
    };
    for op in capture.into_ops() {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        ensure!(
            old_range.start == old_at && new_range.start == new_at,
            "diff op {op:?} does not continue at old {old_at}, new {new_at}"
        );
        old_at = old_range.end;
        new_at = new_range.end;
        if tag == similar::DiffTag::Equal {
            flush(&mut text_at, &mut inserted, &mut deleted)?;
            text_at += chars(&old[old_range]);
        } else {
            inserted.extend(new[new_range].iter().copied());
            deleted += chars(&old[old_range]);
        }
    }
    flush(&mut text_at, &mut inserted, &mut deleted)?;
    let edited = text.to_string();
    ensure!(
        edited == target,
        "minimal diff of {base:?} edited it into {edited:?}, not {target:?}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use loro::LoroDoc;

    use super::*;

    #[test]
    fn transient_3way_merges_disjoint_edits() {
        // base "abc"; disk prepends "X", store appends "Y" → both survive.
        let merger = TransientLoroTextMerge;
        let merged = merger.merge_text("abc", "Xabc", "abcY").unwrap();
        assert_eq!(merged, "XabcY");
    }

    #[test]
    fn transient_3way_keeps_the_side_that_changed() {
        // Non-conflict shapes still round-trip through the merger sanely, though
        // the controller never calls it in these cases (it gates on both-changed):
        let merger = TransientLoroTextMerge;
        // only "theirs" changed (mine == base) → theirs.
        assert_eq!(merger.merge_text("abc", "abcZ", "abc").unwrap(), "abcZ");
        // only "mine" changed (theirs == base) → mine.
        assert_eq!(merger.merge_text("abc", "abc", "Wabc").unwrap(), "Wabc");
    }

    #[test]
    fn a_side_over_the_diff_budget_replaces_the_whole_base() {
        let merged = merge_within("abcdef", "abcdef", "abf", Duration::ZERO).unwrap();
        assert_eq!(merged, "abfabcdef");
    }

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
                let text = crate::mergeable_child::ensure_text(&map, block_id)?;
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
