//! The refusal vocabulary of the `block.promote_task_keyword` compound.
//!
//! A guard that declines to promote is a decision, not an error, so it travels
//! as a value: every outcome of the compound is one of these variants, and the
//! caller reads which one from the op's return payload. That makes a silent
//! refusal unrepresentable.

use std::fmt;

/// Why the engine's guard declined the view-model's promotion proposal. Each
/// variant names the fact that decided it, so the WARN log and the returned
/// payload carry the same disclosure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionRefusal {
    /// The block already carries a `task_state` — promotion is one-shot.
    AlreadyTasked { current: String },
    /// The typed text is not keyword-headed in this document's vocabulary.
    NotKeywordHeaded { vocabulary: Vec<String> },
    /// The prior content was ALREADY keyword-headed, so this edit is not the
    /// not-a-task → task transition. Re-promoting here is the re-derivation bug
    /// the re-ingest reconciler exists to suppress.
    PriorContentAlreadyKeywordHeaded { prior: String },
    /// The guard would promote, but to a different keyword than the caller
    /// proposed — the caller's vocabulary disagrees with the engine's.
    ProposalDisagrees { proposed: String, resolved: String },
}

impl PromotionRefusal {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AlreadyTasked { .. } => "already_tasked",
            Self::NotKeywordHeaded { .. } => "not_keyword_headed",
            Self::PriorContentAlreadyKeywordHeaded { .. } => "prior_content_already_keyword_headed",
            Self::ProposalDisagrees { .. } => "proposal_disagrees",
        }
    }
}

impl fmt::Display for PromotionRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyTasked { current } => {
                write!(f, "block already carries task_state {current:?}")
            }
            Self::NotKeywordHeaded { vocabulary } => write!(
                f,
                "typed text is not headed by a keyword of this document's vocabulary {vocabulary:?}"
            ),
            Self::PriorContentAlreadyKeywordHeaded { prior } => write!(
                f,
                "prior content {prior:?} was already keyword-headed, so this edit is not a \
                 promotion"
            ),
            Self::ProposalDisagrees { proposed, resolved } => write!(
                f,
                "caller proposed keyword {proposed:?} but the document's vocabulary resolves \
                 {resolved:?}"
            ),
        }
    }
}
