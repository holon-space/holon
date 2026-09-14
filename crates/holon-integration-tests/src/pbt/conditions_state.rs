//! What the reference model expects the app to be DISCLOSING.
//!
//! Every degradation Holon raises is a sticky condition on the
//! [`ConditionBus`](holon_api::ConditionBus), keyed by `(subject, kind)`. Until
//! the model tracked them, a transition that fails on purpose proved only that
//! the write was refused — never that the user was TOLD. Those are different
//! defects, and the silent one is the failure the bus exists to prevent: from
//! inside the store it looks exactly like a write that succeeded.
//!
//! The model records the identity, never the prose. The kind is the stable
//! `&'static str` every all-clear site already names; asserting the sentence
//! would make the model a hand-copy of `ConditionKind::detail` and stop being
//! an independent oracle.
//!
//! **Subjects are matched by FILE NAME, anchored to a path component.** Most
//! raise sites carry an absolute path inside the run's temp vault, which no
//! constant can know. The model pins the part it owns — the file name — and the
//! comparison is identity at the final path component, never a raw string
//! suffix: see [`holon_pbt_core::capabilities::subject_matches_expected`] for
//! why (a different file whose name merely ends with the pinned one must not
//! satisfy the expectation).
//!
//! **The model claims authority over KINDS, not over the whole bus.** A booted
//! app legitimately discloses things no transition caused (secrets held in
//! memory, undo history cleared at boot). Comparing the whole set would fail on
//! those, so the invariant compares exactly within the kinds this state has
//! spoken about, and ignores the rest. The exclusion is deliberate and visible
//! here rather than buried in the invariant body.

use std::collections::BTreeSet;

/// One condition the model expects to be in effect: the file NAME that owns its
/// subject, and its stable kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExpectedCondition {
    /// The file name the raise site's subject must END IN — as its final path
    /// component, so a bare file name where the subject is a path, and the
    /// whole subject where it already is a name. Never a raw string suffix: see
    /// [`holon_pbt_core::capabilities::subject_matches_expected`].
    pub subject_name: String,
    pub kind: &'static str,
}

impl ExpectedCondition {
    pub fn matches(&self, subject: &str) -> bool {
        holon_pbt_core::capabilities::subject_matches_expected(subject, &self.subject_name)
    }
}

/// The conditions the reference model believes are raised right now, plus the
/// kinds it claims authority over.
#[derive(Debug, Default, Clone)]
pub struct ConditionsRefState {
    expected: BTreeSet<ExpectedCondition>,
    /// Every kind this state has ever raised or cleared. The invariant judges
    /// only these; a kind never named here is the app's own business.
    governed: BTreeSet<&'static str>,
}

impl ConditionsRefState {
    /// A transition that failed on purpose expects its condition to be in
    /// effect. Idempotent, matching the bus, where a re-raise upserts.
    ///
    /// `subject_name` is the FILE NAME the raise site's subject will carry as
    /// its final path component — not a string suffix.
    pub fn raise(&mut self, subject_name: impl Into<String>, kind: &'static str) {
        self.governed.insert(kind);
        self.expected.insert(ExpectedCondition {
            subject_name: subject_name.into(),
            kind,
        });
    }

    /// The named all-clear happened, so the condition must be gone.
    ///
    /// Governing the kind survives the clear: "this kind must now be absent" is
    /// exactly as much of a claim as "it must be present", and dropping it
    /// would make a failure to clear invisible.
    pub fn clear(&mut self, subject_name: &str, kind: &'static str) {
        self.governed.insert(kind);
        self.expected
            .retain(|c| !(c.kind == kind && c.subject_name == subject_name));
    }

    /// Every condition of `kind`, cleared — for an all-clear that ends a whole
    /// class at once (a clean re-scan, a reconnect of everything).
    pub fn clear_kind(&mut self, kind: &'static str) {
        self.governed.insert(kind);
        self.expected.retain(|c| c.kind != kind);
    }

    pub fn expected(&self) -> &BTreeSet<ExpectedCondition> {
        &self.expected
    }

    /// The kinds the invariant may judge. Empty until a transition speaks, so
    /// a run that exercises no failing transition asserts nothing rather than
    /// asserting the app is quiet.
    pub fn governed(&self) -> &BTreeSet<&'static str> {
        &self.governed
    }
}
