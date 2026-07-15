//! Shared validation helpers for PBT transitions.
//!
//! Replaces the opaque `Option<...>` / `bool` returns of
//! `TransitionFactory::weighted_generator` and
//! `TransitionImpl::preconditions` with `Validated<_, Reason>` so a PBT run
//! can surface *why* each transition was rejected.
//!
//! Rejections accumulate in a thread-local counter and the runner prints a
//! per-transition histogram at end-of-run.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;

use nonempty_collections::NEVec;
use validated::Validated::Good;
use validated::Validated::{self};

/// Named reasons a transition's `weighted_generator` or `preconditions` may
/// return when the state isn't a fit. Used as a histogram key so it must be
/// cheap to clone and hash.
///
/// New transition migrations add variants here. Unmigrated transitions use the
/// catch-all `Unmigrated` / `PreconditionFailed` buckets — coarse but harmless.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Reason {
    // ---------- app / setup ----------
    AppNotStarted,
    AppAlreadyStarted,
    NotProperlySetup,
    PreStartupFileCountZero,

    // ---------- pre-startup invariants ----------
    BlockIdAlreadyExists,
    DirectoryLimitReached,
    VcsAlreadyInitialized,
    LoroDisabledForCorruption,

    // ---------- focus ----------
    NoFocusInMain,
    MainFocusNotSet,
    FocusedIsNotSelf,
    FocusedBlockMissing,
    FocusedNotText,
    FocusedIsPage,
    FocusedNotFocusable,
    FocusedInNoContentUpdate,
    FocusedInLayoutBlocks,
    FocusedNotDescendantOfFocusRoot,
    NoFocusableBlocks,
    SidebarFocusNotRendered,
    /// The LeftSidebar drawer is collapsed, so its page entries aren't
    /// rendered/clickable — a sidebar click-to-focus is impossible until the
    /// drawer is re-opened (a separate `ToggleDrawer` transition). Mirrors
    /// production `columns.rs`, which drops the closed drawer's panel from the
    /// layout and keeps only the toggle.
    LeftSidebarDrawerClosed,

    // ---------- siblings ----------
    NoPreviousSibling,
    NoNextSibling,

    // ---------- editor / atomic editor ----------
    /// The reference owns no editor buffer (`RefLifecycle::has_editor_buffer`
    /// is false) — the editor transitions are inapplicable. Replaces the old
    /// `AtomicEditorDisabled` (env gate) + the editor arm of
    /// `LoroRequiredForAtomicEditor` (storage gate).
    NoEditorBuffer,
    LoroRequiredForAtomicEditor,
    NoActiveEditor,
    EditorContentEmpty,
    AtomicEditorActiveOverride,

    // ---------- chord-ops ----------
    NoGrandparent,
    DragDropDisabled,
    SourceNotRendered,
    BlocksNotInteractiveUnderLayout,
    CyclicParentMove,
    NoOpParentMove,

    // ---------- navigation ----------
    NoNavigationHistory,

    // ---------- task state / toggle ----------
    StateToggleNotApplicable,
    NoTogglableStates,

    // ---------- popup / trigger ----------
    InsufficientTextBlocksForLink,
    BlockIsDefaultLayout,
    InsufficientBlocksForDelete,

    // ---------- peer / sync ----------
    LoroRequiredForPeers,
    PeerLimitReached,
    NoPeersAvailable,
    PeerIndexOutOfBounds,
    PeerBlockMissing,
    PeerEditSourceBlockViolation,

    // ---------- undo / redo ----------
    NoUndoHistory,
    NoRedoHistory,

    // ---------- watch / query ----------
    NoActiveWatches,

    // ---------- bulk / mutation ----------
    NoDocumentsAvailable,
    BlockStateEmpty,
    NoWatchesActive,

    // ---------- pin / sidebar ----------
    NoPinCandidates,
    NoPinsToRemove,

    // ---------- expand_toggle ----------
    NoExpandToggleCandidates,
    NoCollapseToggleCandidates,
    ToggleAlreadyExpanded,
    ToggleNotExpanded,

    // ---------- shared layout-PBT variants ----------
    NoSwitchableHandles,
    NoDrawerHandles,
    NoCollapsibleTargets,
    DeliverNotMeaningfulInBackendTests,

    // ---------- catch-all buckets ----------
    // Used when a transition's gate is hard to name in one variant; prefer a
    // specific variant whenever possible. Both still appear in the histogram
    // and signal where richer decomposition would help.
    Unmigrated,
    PreconditionFailed,
}

/// Map every element of an `NEVec<A>` through `f` into an `NEVec<B>`. Used
/// to translate per-variant `Reason` enums from `holon-layout-testing` /
/// `holon-pbt-core` into this crate's `Reason` enum without losing the
/// nonempty invariant.
pub fn map_nevec<A, B>(src: NEVec<A>, mut f: impl FnMut(A) -> B) -> NEVec<B> {
    let mut iter = src.into_iter();
    let head = iter
        .next()
        .expect("NEVec is statically nonempty — into_iter always yields at least one element");
    let mut out = NEVec::new(f(head));
    for x in iter {
        out.push(f(x));
    }
    out
}

/// Lift a boolean gate into a `Validated`: success carries `()`, failure
/// carries the single named reason.
pub fn check<E>(cond: bool, err: E) -> Validated<(), E> {
    if cond {
        Good(())
    } else {
        Validated::fail(err)
    }
}

/// Bridge for unmigrated transitions whose `weighted_generator` and
/// `preconditions` still return `Option<…>` / `bool` respectively. Once a
/// transition is decomposed into named `Reason` gates, it drops these methods
/// and constructs `Validated` directly. Until then, the catch-all
/// `Reason::Unmigrated` / `Reason::PreconditionFailed` buckets keep the
/// histogram coarse-but-honest.
pub trait LegacyValidationExt<T> {
    fn to_validated_unmigrated(self) -> Validated<T, Reason>;
    fn to_validated_precondition(self) -> Validated<T, Reason>;
}

impl<T> LegacyValidationExt<T> for Option<T> {
    fn to_validated_unmigrated(self) -> Validated<T, Reason> {
        match self {
            Some(t) => Good(t),
            None => Validated::fail(Reason::Unmigrated),
        }
    }
    fn to_validated_precondition(self) -> Validated<T, Reason> {
        match self {
            Some(t) => Good(t),
            None => Validated::fail(Reason::PreconditionFailed),
        }
    }
}

impl LegacyValidationExt<()> for bool {
    fn to_validated_unmigrated(self) -> Validated<(), Reason> {
        check(self, Reason::Unmigrated)
    }
    fn to_validated_precondition(self) -> Validated<(), Reason> {
        check(self, Reason::PreconditionFailed)
    }
}

thread_local! {
    static REJECTION_COUNTER: RefCell<HashMap<(&'static str, Reason), u64>> =
        RefCell::new(HashMap::new());
}

/// Increment the histogram counter once per reason in `reasons`.
pub fn record_rejection(transition: &'static str, reasons: &NEVec<Reason>) {
    REJECTION_COUNTER.with(|c| {
        let mut map = c.borrow_mut();
        for r in reasons.iter() {
            *map.entry((transition, r.clone())).or_insert(0) += 1;
        }
    });
}

/// Drain the histogram and return its entries unsorted.
pub fn take_rejection_histogram() -> Vec<((&'static str, Reason), u64)> {
    REJECTION_COUNTER.with(|c| c.borrow_mut().drain().collect())
}

/// Clear without reading.
pub fn clear_rejection_histogram() {
    REJECTION_COUNTER.with(|c| c.borrow_mut().clear());
}

/// Print the histogram grouped by transition, sorted by descending count, to
/// stderr. Called at end-of-run by the PBT runner.
pub fn print_rejection_histogram() {
    let entries = REJECTION_COUNTER.with(|c| c.borrow().clone());
    if entries.is_empty() {
        return;
    }
    let mut by_transition: BTreeMap<&'static str, Vec<(Reason, u64)>> = BTreeMap::new();
    for ((t, r), count) in entries {
        by_transition.entry(t).or_default().push((r, count));
    }
    eprintln!("\n=== PBT transition rejection histogram ===");
    for (transition, mut rs) in by_transition {
        rs.sort_by(|a, b| b.1.cmp(&a.1));
        let total: u64 = rs.iter().map(|(_, c)| c).sum();
        eprintln!("{transition}: {total} rejections");
        for (r, count) in rs {
            eprintln!("  {r:?} — {count}");
        }
    }
}
