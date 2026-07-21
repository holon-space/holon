//! Undo-across-a-crossing (ADR 0028 §7 / review S6(b)).
//!
//! Undo of a crossing = the **inverse crossing through the same log** — source
//! and target swapped, `widens_audience` negated, a fresh client-minted id, and
//! `inverse_of` pointing at the original. The inverse is appended (Begin +
//! Commit brackets) exactly like any other crossing, so it flows through the
//! one arbitration rule and the one migration path.
//!
//! ## Reject-never-on-undo (senior-review amendment)
//! A LOSING (loud-rejected) crossing must **never** enter the undo stack:
//! undoing a rejected op would apply its inverse to state the op never changed.
//! [`CrossingLog::undo_crossing`] therefore only inverts a crossing that WON
//! arbitration; a loser (or an unknown / uncommitted id) is refused loudly and
//! nothing is appended — state unchanged.

use crate::arbitration::arbitrate;
use crate::log::CrossingLog;
use crate::types::Crossing;
use crate::types::CrossingId;
use crate::types::CrossingKey;
use crate::types::LogEntryBody;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UndoError {
    #[error(
        "cannot undo crossing `{0}`: it was a loud-rejected LOSER of concurrent \
         arbitration (ADR 0028 D2) and never changed committed state — undoing \
         it would apply an inverse to state it never touched"
    )]
    WasRejectedLoser(CrossingId),
    #[error(
        "cannot undo crossing `{0}`: no such committed (winning) crossing in the \
         log"
    )]
    NoCommittedCrossing(CrossingId),
}

impl CrossingLog {
    /// Append the inverse of a committed crossing and return the inverse's key.
    /// Refuses loudly (appending nothing) if `crossing_id` is a rejected loser
    /// or is not a committed winner.
    pub fn undo_crossing(&self, crossing_id: &CrossingId) -> Result<CrossingKey, UndoError> {
        let arb = arbitrate(&self.entries());

        // A rejected loser must never enter the undo stack.
        if arb.is_loser(crossing_id) {
            return Err(UndoError::WasRejectedLoser(crossing_id.clone()));
        }

        let winner = arb
            .winners
            .iter()
            .find(|c| &c.crossing_id == crossing_id)
            .ok_or_else(|| UndoError::NoCommittedCrossing(crossing_id.clone()))?;

        let inverse = Crossing {
            crossing_id: CrossingId(format!("{}~undo", crossing_id.0)),
            block: winner.block.clone(),
            source: winner.target.clone(),
            target: winner.source.clone(),
            widens_audience: !winner.widens_audience,
            content: winner.content.clone(),
            inverse_of: Some(crossing_id.clone()),
            // The inverse supersedes the crossing it reverses, so after the undo
            // only the inverse is live for the block (the original is history).
            supersedes: Some(crossing_id.clone()),
        };
        let inverse_id = inverse.crossing_id.clone();
        let key = self.append_entry(LogEntryBody::CrossingBegin(inverse));
        self.append_entry(LogEntryBody::CrossingCommit {
            crossing_id: inverse_id,
        });
        Ok(key)
    }
}
