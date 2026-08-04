//! Debug-only differential shadow of the org write-back document cache.
//!
//! Option C replaces the hand-maintained `doc_blocks` cache in
//! `FileSyncController` with the output of the declared `home_by` combinator.
//! Before that cutover, this module runs the combinator **in parallel** with
//! the existing cache and compares them, so every debug-build run of the
//! keystone is a differential test of the replacement against the incumbent.
//!
//! # Why the comparison is at quiescence, not per event
//!
//! `home_by` and the write-back resolver are independent subscriptions to the
//! same feed, delivered on independent schedules. Comparing after each
//! `on_block_changed` would compare two different feed prefixes and report
//! divergence that is really just lag. The derived-data contract says the two
//! must agree **once inputs quiesce**, so that is what is asserted: a
//! comparison runs only when neither subscription has produced anything since
//! the previous check, and only a mismatch that survives **two** consecutive
//! such checks is genuine non-convergence.
//!
//! # Root membership
//!
//! The per-document member list this exposes **excludes the document root**,
//! matching `get_blocks`/`doc_blocks`, so it is a drop-in for their consumers.
//! The root stays in the combinator's accumulator, where page-ness toggles are
//! detected.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering as AtomicOrdering;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::live_data::LiveData;
use holon_api::live_data::home_by::HomedDiff;
use holon_core::block_ordering::BlockOrdering;
use holon_filesystem::BlockReader;

/// Consecutive quiescent checks a mismatch must survive before it is called
/// non-convergence. A real divergence never converges, so it survives any
/// finite limit — this trades detection LATENCY for immunity to a convergence
/// window that is bounded but not instantaneous. Measured: a lagging feed never
/// exceeds a streak of 1 (§9.8 of the Option C design doc).
const MISMATCH_STREAK_LIMIT: u32 = 8;

/// Arming policy. The shadow is DEFAULT-ON in debug builds so every debug run is
/// a free differential surface. Suites whose oracles measure PROD read behaviour
/// must opt out: the shadow issues authority reads of its own, and the
/// read-budget oracle cannot tell measurement apparatus from production work.
/// See the Option C design doc §9.8.
static SHADOW_ARMED: AtomicBool = AtomicBool::new(true);

/// Comparisons that actually COMPLETED (system was quiescent). Liveness
/// evidence: a shadow that never completes a comparison proves nothing, so the
/// armed differential suite gates on this being non-zero.
static COMPLETED_COMPARISONS: AtomicU64 = AtomicU64::new(0);

/// Per-DOCUMENT comparisons performed. This is the observable worth gating on:
/// `COMPLETED_COMPARISONS` counts tick-loop iterations and rises even when the
/// controller has cached no documents yet, so a non-zero value there does NOT
/// prove any holder content was ever checked against anything.
static DOCS_COMPARED: AtomicU64 = AtomicU64::new(0);

/// Set before the non-convergence panic. The panic happens on a spawned task,
/// which does NOT fail a plain `#[test]` — the composed harness catches it via
/// `inv-no-observed-errors`, but the armed differential suite has no such
/// invariant, so without this a divergence there would be swallowed and the
/// regression gate would be decorative.
static DIVERGENCE_DETECTED: AtomicBool = AtomicBool::new(false);

/// Opt this process out of the shadow. Called at suite setup by harnesses whose
/// oracles measure prod read behaviour.
pub fn disable_for_budget_suite() {
    SHADOW_ARMED.store(false, AtomicOrdering::Relaxed);
}

/// Whether the shadow should be wired in this process. `HOLON_SHADOW_OFF` is an
/// additional ad-hoc lever for measuring the shadow's own interference.
pub fn is_armed() -> bool {
    SHADOW_ARMED.load(AtomicOrdering::Relaxed) && std::env::var_os("HOLON_SHADOW_OFF").is_none()
}

/// Number of quiescent comparisons that ran to completion in this process.
pub fn completed_comparisons() -> u64 {
    COMPLETED_COMPARISONS.load(AtomicOrdering::Relaxed)
}

/// Number of per-DOCUMENT comparisons performed in this process. Non-zero
/// proves a real document's holder content was checked against `doc_blocks`.
pub fn docs_compared() -> u64 {
    DOCS_COMPARED.load(AtomicOrdering::Relaxed)
}

/// Whether any non-convergence was detected in this process.
pub fn divergence_detected() -> bool {
    DIVERGENCE_DETECTED.load(AtomicOrdering::Relaxed)
}

use crate::home_authority::BlockHomeAuthority;
use crate::home_authority::DocHome;

/// Seams the shadow needs. Constructed only in debug builds.
pub struct ShadowInputs {
    pub feed: Arc<LiveData<Block>>,
    pub reader: Arc<dyn BlockReader>,
    pub ordering: Arc<dyn BlockOrdering>,
}

struct Member {
    parent: Option<String>,
    prev: Option<String>,
}

/// The shadow holder plus the quiescence state machine.
pub struct WritebackShadow {
    /// document -> its members, folded from the emitted `HomedDiff`s.
    holder: BTreeMap<DocHome, BTreeMap<String, Member>>,
    /// Set by any feed activity on either subscription since the last check.
    activity: bool,
    /// Consecutive quiescent checks that found the same mismatch.
    mismatch_streak: u32,
    /// The controller-side snapshot seen at the previous check. `doc_blocks` is
    /// also mutated by paths the feed never touches — file ingest, the boot
    /// scan, `poll_tracked_files`, `re_render_all_tracked` — so feed silence
    /// alone does NOT mean the system is quiescent. Quiescence requires BOTH
    /// sides to have stopped moving.
    last_tracked: Option<Vec<(EntityUri, Vec<(EntityUri, EntityUri)>)>>,
    /// Proof-of-execution counters: a shadow that never ran is not evidence.
    checks_run: u64,
    docs_compared: u64,
}

impl Default for WritebackShadow {
    fn default() -> Self {
        Self::new()
    }
}

impl WritebackShadow {
    pub fn new() -> Self {
        Self {
            holder: BTreeMap::new(),
            activity: false,
            mismatch_streak: 0,
            last_tracked: None,
            checks_run: 0,
            docs_compared: 0,
        }
    }

    /// Spawn the combinator, forwarding its emissions over a channel.
    ///
    /// Forwarding through a channel (rather than polling the stream inside the
    /// controller's `select!`) keeps the holder and the stream separately
    /// borrowable. A diff still in flight at check time simply shows up as
    /// activity on a later tick, which resets the streak — the two-check rule
    /// already tolerates it.
    pub fn spawn(
        inputs: ShadowInputs,
    ) -> tokio::sync::mpsc::UnboundedReceiver<HomedDiff<DocHome, Block>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            use futures::StreamExt as _;
            let authority = Arc::new(BlockHomeAuthority::new(inputs.reader, inputs.ordering));
            let mut stream = std::pin::pin!(inputs.feed.home_by(authority));
            while let Some(item) = stream.next().await {
                match item {
                    Ok(diff) => {
                        if tx.send(diff).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        // The combinator's error latch is terminal by design
                        // ("let it die"): supervision is an Inc-2 concern. In a
                        // debug build the shadow simply stops, loudly.
                        tracing::error!(
                            "[writeback-shadow] home_by stream errored and ENDED: {e:#} — the \
                             shadow is now blind for the rest of this process"
                        );
                        return;
                    }
                }
            }
        });
        rx
    }

    pub fn note_activity(&mut self) {
        self.activity = true;
    }

    pub fn apply(&mut self, diff: HomedDiff<DocHome, Block>) {
        self.activity = true;
        match diff {
            HomedDiff::Upsert {
                doc,
                key,
                prev,
                value,
            } => {
                let parent = if value.parent_id == EntityUri::no_parent() {
                    None
                } else {
                    Some(value.parent_id.as_str().to_string())
                };
                self.holder
                    .entry(doc)
                    .or_default()
                    .insert(key, Member { parent, prev });
            }
            HomedDiff::Remove { doc, key } => {
                if let Some(members) = self.holder.get_mut(&doc) {
                    members.remove(&key);
                    if members.is_empty() {
                        self.holder.remove(&doc);
                    }
                }
            }
        }
    }

    /// The document's members grouped `parent -> [child ids]`, each group in
    /// the holder's document-relative sibling order, EXCLUDING the document
    /// root. Per-parent rather than one flat sequence — see [`Self::compare`].
    fn materialize_by_parent(&self, doc: &DocHome) -> BTreeMap<String, Vec<String>> {
        let Some(members) = self.holder.get(doc) else {
            return BTreeMap::new();
        };
        let root = match doc {
            DocHome::Resolved(r) => Some(r.as_str().to_string()),
            DocHome::Unresolved => None,
        };

        let mut by_parent: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, m) in members {
            // Children-only: the page is not a member of its own document.
            if Some(id.clone()) == root {
                continue;
            }
            by_parent
                .entry(m.parent.clone().unwrap_or_default())
                .or_default()
                .push(id.clone());
        }
        for group in by_parent.values_mut() {
            *group = order_group(group, members);
        }
        by_parent
    }

    /// Compare every document the controller has cached against the shadow,
    /// **per parent**. Returns a diagnosable report for the first divergence.
    ///
    /// # Why per-parent and not a flat sequence
    ///
    /// `doc_blocks` is seeded from `get_blocks`, whose `ORDER BY sort_key, id`
    /// is a GLOBAL sort over fractional indices minted **per sibling group** —
    /// every group restarts at the same low key, so rows from different parents
    /// tie and fall back to the `id` tiebreak. Its `Vec` is a *set with
    /// within-parent relative order*, never a document sequence. Comparing it
    /// against the holder's genuine document order was invalid by construction
    /// and reported a defect in THIS ORACLE as a defect in prod. Within-parent
    /// order is the only claim both sides make, and the only one prod reads —
    /// `render_entitys` re-nests by `parent_id`.
    ///
    /// Ids only, deliberately: the shadow's values come from the (lagging)
    /// feed while `doc_blocks`' come from authoritative reads, so equal
    /// membership and order can legitimately pair with different content.
    /// Comparing values would report lag as divergence.
    fn compare(&mut self, tracked: &[(EntityUri, Vec<(EntityUri, EntityUri)>)]) -> Option<String> {
        for (doc, cached) in tracked {
            self.docs_compared += 1;
            DOCS_COMPARED.fetch_add(1, AtomicOrdering::Relaxed);
            let shadow = self.materialize_by_parent(&DocHome::Resolved(doc.clone()));

            let mut cache_by_parent: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (id, parent) in cached {
                let anchor = if *parent == EntityUri::no_parent() {
                    String::new()
                } else {
                    parent.as_str().to_string()
                };
                cache_by_parent
                    .entry(anchor)
                    .or_default()
                    .push(id.as_str().to_string());
            }

            if shadow == cache_by_parent {
                continue;
            }
            let parents: BTreeSet<&String> = shadow.keys().chain(cache_by_parent.keys()).collect();
            for parent in parents {
                let empty = Vec::new();
                let s = shadow.get(parent).unwrap_or(&empty);
                let c = cache_by_parent.get(parent).unwrap_or(&empty);
                if s == c {
                    continue;
                }
                let at = s
                    .iter()
                    .zip(c.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or_else(|| s.len().min(c.len()));
                // Where did the blocks the cache has but the holder lacks
                // actually go? A membership gap is either "the holder never
                // saw it" or "the holder homed it to a DIFFERENT document" —
                // radically different defects, so name which.
                let s_set: BTreeSet<&String> = s.iter().collect();
                let mut elsewhere = Vec::new();
                for id in c.iter().filter(|id| !s_set.contains(id)) {
                    let found: Vec<String> = self
                        .holder
                        .iter()
                        .filter(|(_, members)| members.contains_key(id))
                        .map(|(d, _)| format!("{d:?}"))
                        .collect();
                    elsewhere.push(if found.is_empty() {
                        format!("{id} = ABSENT FROM HOLDER ENTIRELY")
                    } else {
                        format!("{id} = homed to {found:?}")
                    });
                }
                return Some(format!(
                    "doc={doc}\n  parent={parent}\n  first divergent index within parent: {at}\n  \
                     home_by holder ({} children): {s:?}\n  \
                     doc_blocks cache ({} children): {c:?}\n  \
                     cache-only blocks: {elsewhere:?}",
                    s.len(),
                    c.len(),
                ));
            }
        }
        None
    }

    /// Run one quiescence-gated check. Panics on genuine non-convergence.
    ///
    /// Returns `true` when a comparison actually ran (i.e. the system was
    /// quiescent), so callers can prove the shadow executed rather than infer
    /// it from the absence of a panic.
    pub fn check(&mut self, tracked: &[(EntityUri, Vec<(EntityUri, EntityUri)>)]) -> bool {
        // Feed side moved?
        if self.activity {
            self.activity = false;
            self.mismatch_streak = 0;
            self.last_tracked = Some(tracked.to_vec());
            return false;
        }
        // Controller side moved? `doc_blocks` changes on file-ingest / scan /
        // poll / full-rerender paths that produce no feed event at all, so
        // without this a reseed from disk looks like divergence.
        if self.last_tracked.as_deref() != Some(tracked) {
            self.mismatch_streak = 0;
            self.last_tracked = Some(tracked.to_vec());
            return false;
        }
        self.checks_run += 1;
        COMPLETED_COMPARISONS.fetch_add(1, AtomicOrdering::Relaxed);
        match self.compare(tracked) {
            None => {
                self.mismatch_streak = 0;
            }
            Some(report) => {
                self.mismatch_streak += 1;
                if self.mismatch_streak >= MISMATCH_STREAK_LIMIT {
                    DIVERGENCE_DETECTED.store(true, AtomicOrdering::Relaxed);
                    panic!(
                        "[writeback-shadow] NON-CONVERGENCE: the home_by holder and the \
                         hand-maintained doc_blocks cache disagree after {MISMATCH_STREAK_LIMIT} \
                         consecutive quiescent checks with no feed activity between them. This \
                         is a real divergence, not lag.\n{report}\n\
                         (Option C Inc 1 differential shadow; debug builds only.)"
                    );
                }
                tracing::warn!(
                    "[writeback-shadow] mismatch on quiescent check {} (streak {}): {report}",
                    self.checks_run,
                    self.mismatch_streak
                );
            }
        }
        true
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.checks_run, self.docs_compared)
    }
}

/// Order one sibling group by following its previous-sibling chain. A chain
/// that forks or cycles terminates and appends the unreachable members sorted,
/// so a defect surfaces as a comparison mismatch rather than a hang.
fn order_group(group: &[String], members: &BTreeMap<String, Member>) -> Vec<String> {
    let set: BTreeSet<&String> = group.iter().collect();
    let mut succ: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    for id in group {
        let prev = members
            .get(id)
            .and_then(|m| m.prev.clone())
            .filter(|p| set.contains(p));
        succ.entry(prev).or_default().push(id.clone());
    }
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let Some(next) = succ.get(&cursor).and_then(|n| n.iter().min().cloned()) else {
            break;
        };
        if !seen.insert(next.clone()) {
            break;
        }
        out.push(next.clone());
        cursor = Some(next);
    }
    let mut leftovers: Vec<String> = group
        .iter()
        .filter(|g| !seen.contains(*g))
        .cloned()
        .collect();
    leftovers.sort();
    out.extend(leftovers);
    out
}
