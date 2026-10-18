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
    /// is false) — the editor transitions are inapplicable. A transition that
    /// also needs the editor cell gates on `EditorCellRequired`.
    NoEditorBuffer,
    /// The drawn editor leg commits through the dispatcher, so no editor
    /// cell (`MutableText`) resolves (`RefLifecycle::editor_cell_attached`).
    EditorCellRequired,
    NoActiveEditor,
    EditorContentEmpty,
    AtomicEditorActiveOverride,

    // ---------- chord-ops ----------
    NoGrandparent,
    /// Outdenting a direct page child is refused by the op engine (ADR 0028
    /// D1).
    ParentIsPage,
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
    /// The targeted block surfaces no `view_mode_switcher`, so the VMS button
    /// a `SwitchViewMode` would click does not exist and the click cannot take
    /// effect. Distinct from `NoSwitchableHandles`, which is the *generator*
    /// declining a state with no candidates at all.
    NoModeSwitchableSurface,
    /// The targeted block IS mode-switchable, but does not offer the requested
    /// mode — its VMS has no button with that name.
    ModeNotOfferedByBlock,
    NoDrawerHandles,
    NoCollapsibleTargets,
    DeliverNotMeaningfulInBackendTests,
    /// A dispatch hold or injected failure: drawn at random it would park an
    /// op no later step releases, so only a hand-authored case names one.
    HandAuthoredOnly,

    // ---------- true sharing (two-instance) ----------
    /// The vault is already shared with the receiver — re-sharing changes
    /// nothing and would burn a tick the sequence needs for syncing.
    VaultAlreadyShared,

    // ---------- external cut & paste ----------
    /// A block is on disk in two files, and the model does not describe this
    /// transition while one is (D229.b).
    CopyStands,
    /// No block is on disk in two files.
    NoCopyStands,

    // ---------- integration connect timing ----------
    /// The integration peer was not drawn to wait for a release, or already
    /// answers.
    IntegrationPeerNotHeld,

    // ---------- catch-all buckets ----------
    // Used when a transition's gate is hard to name in one variant; prefer a
    // specific variant whenever possible. Both still appear in the histogram
    // and signal where richer decomposition would help.
    Unmigrated,
    PreconditionFailed,
}

/// What a decline says about the alphabet, for `inv-alphabet-coverage`: a
/// surface the composition must render is missing EVERYWHERE so the kind left
/// the alphabet, the state has not reached the shape the kind needs, or this
/// medium never generates the kind at all.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ReasonClass {
    /// The kind can never be served while the composition renders what it
    /// renders. Promised by the wiring and only ever declined this way, the
    /// kind is a silent coverage hole.
    AlphabetGone,
    /// This state is no fit; another state in the same run can be.
    PreconditionUnmet,
    /// The generator declines by construction — it ignores the state and fails
    /// for every one. The kind is drawn by another suite (a hand-authored
    /// regression step, another medium's PBT), so its alphabet is not gone;
    /// `alphabet_coverage::EXPECTED_ABSENT` names what accounts for each one.
    NotDrawnHere,
}

impl Reason {
    /// Exhaustive by construction: a new `Reason` does not compile until it is
    /// classified. `AlphabetGone` is grown one variant at a time, each promoted
    /// only once a run proves the reason is state-INDEPENDENT — a reason that
    /// merely looks like a missing surface (`NoSwitchableHandles`,
    /// `NoModeSwitchableSurface`) still fires in states that simply have not
    /// built the shape yet. A reason a generator returns without binding the
    /// state at all is `NotDrawnHere`, not `AlphabetGone`: the kind is drawn
    /// elsewhere, so nothing is missing from the composition.
    pub fn class(&self) -> ReasonClass {
        match self {
            // No block in the whole composition renders a `state_toggle`
            // widget, so no state can offer a toggle (the 2026-07-05
            // `ToggleState` vanish).
            Reason::NoTogglableStates => ReasonClass::AlphabetGone,
            // Returned by a `weighted_generator(_: &R)` that never binds the
            // state: the kind declines in every state of every run, and what
            // accounts for it is its `EXPECTED_ABSENT` claim.
            Reason::HandAuthoredOnly | Reason::DeliverNotMeaningfulInBackendTests => {
                ReasonClass::NotDrawnHere
            }
            Reason::AppNotStarted
            | Reason::AppAlreadyStarted
            | Reason::NotProperlySetup
            | Reason::PreStartupFileCountZero
            | Reason::BlockIdAlreadyExists
            | Reason::DirectoryLimitReached
            | Reason::VcsAlreadyInitialized
            | Reason::LoroDisabledForCorruption
            | Reason::NoFocusInMain
            | Reason::MainFocusNotSet
            | Reason::FocusedIsNotSelf
            | Reason::FocusedBlockMissing
            | Reason::FocusedNotText
            | Reason::FocusedIsPage
            | Reason::FocusedNotFocusable
            | Reason::FocusedInNoContentUpdate
            | Reason::FocusedInLayoutBlocks
            | Reason::FocusedNotDescendantOfFocusRoot
            | Reason::NoFocusableBlocks
            | Reason::SidebarFocusNotRendered
            | Reason::LeftSidebarDrawerClosed
            | Reason::NoPreviousSibling
            | Reason::NoNextSibling
            | Reason::NoEditorBuffer
            | Reason::EditorCellRequired
            | Reason::NoActiveEditor
            | Reason::EditorContentEmpty
            | Reason::AtomicEditorActiveOverride
            | Reason::NoGrandparent
            | Reason::ParentIsPage
            | Reason::DragDropDisabled
            | Reason::SourceNotRendered
            | Reason::BlocksNotInteractiveUnderLayout
            | Reason::CyclicParentMove
            | Reason::NoOpParentMove
            | Reason::NoNavigationHistory
            | Reason::StateToggleNotApplicable
            | Reason::InsufficientTextBlocksForLink
            | Reason::BlockIsDefaultLayout
            | Reason::InsufficientBlocksForDelete
            | Reason::LoroRequiredForPeers
            | Reason::PeerLimitReached
            | Reason::NoPeersAvailable
            | Reason::PeerIndexOutOfBounds
            | Reason::PeerBlockMissing
            | Reason::PeerEditSourceBlockViolation
            | Reason::NoUndoHistory
            | Reason::NoRedoHistory
            | Reason::NoActiveWatches
            | Reason::NoDocumentsAvailable
            | Reason::BlockStateEmpty
            | Reason::NoWatchesActive
            | Reason::NoPinCandidates
            | Reason::NoPinsToRemove
            | Reason::NoExpandToggleCandidates
            | Reason::NoCollapseToggleCandidates
            | Reason::ToggleAlreadyExpanded
            | Reason::ToggleNotExpanded
            | Reason::NoSwitchableHandles
            | Reason::NoModeSwitchableSurface
            | Reason::ModeNotOfferedByBlock
            | Reason::NoDrawerHandles
            | Reason::NoCollapsibleTargets
            | Reason::VaultAlreadyShared
            | Reason::CopyStands
            | Reason::NoCopyStands
            | Reason::IntegrationPeerNotHeld
            | Reason::Unmigrated
            | Reason::PreconditionFailed => ReasonClass::PreconditionUnmet,
        }
    }
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
    if cond { Good(()) } else { Validated::fail(err) }
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
        rs.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        let total: u64 = rs.iter().map(|(_, c)| c).sum();
        eprintln!("{transition}: {total} rejections");
        for (r, count) in rs {
            eprintln!("  {r:?} — {count}");
        }
    }
}

#[cfg(test)]
mod reason_class_state_independence {
    use super::Reason;
    use super::ReasonClass;

    /// A reason whose only producers are `fn weighted_generator(_: &R)` — the
    /// state is not even bound — is state-INDEPENDENT by signature, so the
    /// kind never reaches the generated alphabet of this medium.
    /// `HandAuthoredOnly` is produced exactly that way by
    /// `RebootWithBacklogHeld`, `ReleaseBacklog`, `ReleaseHeld`,
    /// `StructuralKeyBurst`, `DelayAdmissions`, `FailNextDispatch` and
    /// `HoldDispatch`; `DeliverNotMeaningfulInBackendTests` by
    /// `DeliverBlockContent`.
    #[test]
    fn a_reason_no_state_can_satisfy_is_not_drawn_here() {
        assert_eq!(
            Reason::HandAuthoredOnly.class(),
            ReasonClass::NotDrawnHere,
            "HandAuthoredOnly is returned by weighted_generator(_: &R) with the state unbound"
        );
        assert_eq!(
            Reason::DeliverNotMeaningfulInBackendTests.class(),
            ReasonClass::NotDrawnHere,
            "DeliverNotMeaningfulInBackendTests is returned with the state unbound"
        );
    }
}
