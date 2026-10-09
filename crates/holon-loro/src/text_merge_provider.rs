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

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use anyhow::ensure;
use holon_filesystem::TextMergeOutcome;
use holon_filesystem::ThreeWayTextMerge;
use loro::ExportMode;
use loro::LoroDoc;
use loro::LoroText;
use similar::DiffOp;
use similar::DiffTag;
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
/// When either side's diff costs more than `work_limit`, the outcome is
/// [`TextMergeOutcome::TooLarge`] and nothing is merged.
///
/// Wired into `FileSyncController` in Direct mode via
/// `FileSyncController::with_text_merge`. In Full (Loro-the-store) mode this is
/// never invoked — the live CRDT already merges concurrent edits.
pub struct TransientLoroTextMerge {
    work_limit: u64,
}

impl Default for TransientLoroTextMerge {
    fn default() -> Self {
        Self {
            work_limit: WORK_LIMIT,
        }
    }
}

impl TransientLoroTextMerge {
    pub fn with_work_limit(work_limit: u64) -> Self {
        Self { work_limit }
    }
}

/// Loro orders concurrent inserts at one position lower peer id first, which
/// puts `mine` before `theirs`.
const MINE_PEER: u64 = 1;
const THEIRS_PEER: u64 = 2;

/// Work one side's diff may cost: Myers over `n` old and `m` new clusters with
/// edit distance `d` costs `(n + m + 1) * (d + 1)`.
const WORK_LIMIT: u64 = 20_000_000;

impl ThreeWayTextMerge for TransientLoroTextMerge {
    fn merge_text(&self, base: &str, theirs: &str, mine: &str) -> Result<TextMergeOutcome> {
        let (Some(theirs_runs), Some(mine_runs)) = (
            edit_runs(base, theirs, self.work_limit)?,
            edit_runs(base, mine, self.work_limit)?,
        ) else {
            return Ok(TextMergeOutcome::TooLarge);
        };

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

        let fork = |peer: u64, runs: &[Run], side: &str, label: &str| -> Result<LoroDoc> {
            let doc = LoroDoc::new();
            doc.set_peer_id(peer)
                .with_context(|| format!("transient merge: set {label} peer id"))?;
            doc.import(&snapshot)
                .with_context(|| format!("transient merge: import base into {label}"))?;
            let text = doc.get_text("content");
            apply_runs(&text, runs).with_context(|| format!("transient merge: apply {label}"))?;
            let edited = text.to_string();
            ensure!(
                edited == side,
                "the diff of {label} edited {base:?} into {edited:?}, not {side:?}"
            );
            doc.commit();
            Ok(doc)
        };
        let theirs_doc = fork(THEIRS_PEER, &theirs_runs, theirs, "theirs")?;
        let mine_doc = fork(MINE_PEER, &mine_runs, mine, "mine")?;

        let mine_updates = mine_doc
            .export(ExportMode::all_updates())
            .context("transient merge: export mine updates")?;
        theirs_doc
            .import(&mine_updates)
            .context("transient merge: merge mine into theirs")?;
        Ok(TextMergeOutcome::Merged(
            theirs_doc.get_text("content").to_string(),
        ))
    }
}

/// One run of a side's edit of `base`. Counts are in chars, as `LoroText`
/// indexes.
#[derive(Debug)]
enum Run<'a> {
    Keep(usize),
    Change { deleted: usize, inserted: &'a str },
}

/// The runs of the minimal diff of grapheme clusters that edits `base` into
/// `target`, so a cluster's chars are kept, deleted or replaced as one unit.
/// `None` when the diff costs more than `work_limit`.
///
/// `LoroText::update` is not minimal: it may delete a char and insert an equal
/// one, and the other side's delete of the old char then misses the new one.
/// A diff of lines first is not minimal either: matching equal lines, it may
/// replace a char the minimal diff keeps.
fn edit_runs<'a>(base: &str, target: &'a str, work_limit: u64) -> Result<Option<Vec<Run<'a>>>> {
    let old = Clusters::new(base);
    let new = Clusters::new(target);
    let mut ids = HashMap::new();
    let old_ids = intern(&mut ids, &old);
    let new_ids = intern(&mut ids, &new);
    let Some(ops) = bounded_diff(&old_ids, &new_ids, work_limit) else {
        return Ok(None);
    };
    let mut runs = Vec::with_capacity(ops.len());
    for (tag, old_range, new_range) in walk(&ops, old.len(), new.len())? {
        let deleted = old.slice(old_range).chars().count();
        runs.push(match tag {
            DiffTag::Equal => Run::Keep(deleted),
            _ => Run::Change {
                deleted,
                inserted: new.slice(new_range),
            },
        });
    }
    Ok(Some(runs))
}

/// The clusters of `text` as ids, equal iff the clusters are.
fn intern<'t>(ids: &mut HashMap<&'t str, u32>, text: &Clusters<'t>) -> Vec<u32> {
    (0..text.len())
        .map(|i| {
            let next = ids.len() as u32;
            *ids.entry(text.slice(i..i + 1)).or_insert(next)
        })
        .collect()
}

/// A text cut into its grapheme clusters.
struct Clusters<'a> {
    text: &'a str,
    /// Byte offset of each cluster's start, then the text's length.
    bounds: Vec<usize>,
}

impl<'a> Clusters<'a> {
    fn new(text: &'a str) -> Self {
        let mut bounds: Vec<usize> = text.grapheme_indices(true).map(|(at, _)| at).collect();
        bounds.push(text.len());
        Self { text, bounds }
    }

    fn len(&self) -> usize {
        self.bounds.len() - 1
    }

    fn slice(&self, clusters: Range<usize>) -> &'a str {
        &self.text[self.bounds[clusters.start]..self.bounds[clusters.end]]
    }
}

/// The ops of a diff of `old_len` into `new_len` items, checked to continue
/// one another and to cover both. They come from the raw Myers hook:
/// `similar::capture_diff_slices` also compacts them, and its ops' positions
/// then disagree with their order when items repeat.
fn walk(
    ops: &[DiffOp],
    old_len: usize,
    new_len: usize,
) -> Result<Vec<(DiffTag, Range<usize>, Range<usize>)>> {
    let (mut old_at, mut new_at) = (0, 0);
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        ensure!(
            old_range.start == old_at && new_range.start == new_at,
            "diff op {op:?} does not continue at old {old_at}, new {new_at}"
        );
        old_at = old_range.end;
        new_at = new_range.end;
        out.push((tag, old_range, new_range));
    }
    ensure!(
        old_at == old_len && new_at == new_len,
        "diff ops end at old {old_at}, new {new_at}, not {old_len}, {new_len}"
    );
    Ok(out)
}

/// The minimal diff of `old` into `new`, or `None` when it costs more than
/// `work_limit`.
fn bounded_diff<T: PartialEq>(old: &[T], new: &[T], work_limit: u64) -> Option<Vec<DiffOp>> {
    let span = (old.len() + new.len() + 1) as u64;
    edit_distance_at_most(old, new, (work_limit / span).checked_sub(1)?)?;
    let mut capture = similar::algorithms::Capture::new();
    let Ok(()) =
        similar::algorithms::myers::diff(&mut capture, old, 0..old.len(), new, 0..new.len());
    Some(capture.into_ops())
}

/// The edit distance of `a` and `b` when it is at most `max_d`: Myers' greedy
/// forward search, costing `O((n + m) * d)`.
fn edit_distance_at_most<T: PartialEq>(a: &[T], b: &[T], max_d: u64) -> Option<u64> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max_d = max_d.min((a.len() + b.len()) as u64) as isize;
    let offset = max_d + 1;
    let mut furthest = vec![0isize; 2 * max_d as usize + 3];
    for d in 0..=max_d {
        for k in (-d..=d).step_by(2) {
            let i = (offset + k) as usize;
            let mut x = if k == -d || (k != d && furthest[i - 1] < furthest[i + 1]) {
                furthest[i + 1]
            } else {
                furthest[i - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            furthest[i] = x;
            if x >= n && y >= m {
                return Some(d as u64);
            }
        }
    }
    None
}

/// Applies `runs` to `text`. A changed run is held until the next kept run,
/// then inserts before it deletes, so its text sits in the gap before the
/// chars it replaces.
fn apply_runs(text: &LoroText, runs: &[Run]) -> Result<()> {
    let (mut at, mut inserted, mut deleted) = (0, String::new(), 0);
    let flush = |at: &mut usize, inserted: &mut String, deleted: &mut usize| -> Result<()> {
        text.insert(*at, inserted)?;
        *at += inserted.chars().count();
        text.delete(*at, *deleted)?;
        inserted.clear();
        *deleted = 0;
        Ok(())
    };
    for run in runs {
        match run {
            Run::Keep(chars) => {
                flush(&mut at, &mut inserted, &mut deleted)?;
                at += chars;
            }
            Run::Change {
                deleted: chars,
                inserted: piece,
            } => {
                deleted += chars;
                inserted.push_str(piece);
            }
        }
    }
    flush(&mut at, &mut inserted, &mut deleted)
}

#[cfg(test)]
mod tests {
    use loro::LoroDoc;

    use super::*;

    fn merged(text: &str) -> TextMergeOutcome {
        TextMergeOutcome::Merged(text.to_string())
    }

    #[test]
    fn transient_3way_merges_disjoint_edits() {
        // base "abc"; disk prepends "X", store appends "Y" → both survive.
        let merger = TransientLoroTextMerge::default();
        let outcome = merger.merge_text("abc", "Xabc", "abcY").unwrap();
        assert_eq!(outcome, merged("XabcY"));
    }

    #[test]
    fn transient_3way_keeps_the_side_that_changed() {
        // Non-conflict shapes still round-trip through the merger sanely, though
        // the controller never calls it in these cases (it gates on both-changed):
        let merger = TransientLoroTextMerge::default();
        // only "theirs" changed (mine == base) → theirs.
        assert_eq!(
            merger.merge_text("abc", "abcZ", "abc").unwrap(),
            merged("abcZ")
        );
        // only "mine" changed (theirs == base) → mine.
        assert_eq!(
            merger.merge_text("abc", "abc", "Wabc").unwrap(),
            merged("Wabc")
        );
    }

    #[test]
    fn edit_distance_is_found_up_to_the_cap() {
        let (a, b) = (b"abcabba".as_slice(), b"cbabac".as_slice());
        assert_eq!(edit_distance_at_most(a, b, 5), Some(5));
        assert_eq!(edit_distance_at_most(a, b, 4), None);
        assert_eq!(edit_distance_at_most(b"", b"", 0), Some(0));
        assert_eq!(edit_distance_at_most(b"ab", b"", 1), None);
    }

    #[test]
    fn a_side_over_the_work_limit_merges_nothing() {
        // (6 + 3 + 1) * (3 + 1)
        let limit = 40;
        let merge = |limit| {
            TransientLoroTextMerge::with_work_limit(limit)
                .merge_text("abcdef", "abcdef", "abf")
                .unwrap()
        };
        assert_eq!(merge(limit), merged("abf"));
        assert_eq!(merge(limit - 1), TextMergeOutcome::TooLarge);
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
