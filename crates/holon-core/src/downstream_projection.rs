//! `DownstreamProjection` — the convergent feed from the consolidator to a
//! sink.
//!
//! In the target block-sync architecture, source-peers (org, markdown, UI)
//! send *intents* up to the leading merge component (Loro when present), and
//! the consolidator publishes its merged state *down* to sinks (Turso/SQL,
//! org, markdown). This trait is the pull edge of that downstream feed: a
//! caller can ask the consolidator to project its accumulated changes to the
//! SQL sink synchronously.
//!
//! It exists because, during the initial org scan, the continuously-running
//! downstream projector (`LoroSyncController::on_loro_changed`) is not
//! subscribed yet (the controller starts post-scan). The org reconciler sends
//! create/relocate intents into Loro and then `flush()`es so the `block_raw`
//! rows are written by the *one* legitimate sink-writer (the projection),
//! never by the source-peer itself.

use async_trait::async_trait;

use crate::traits::Result;

/// What one projection pass achieved.
///
/// A pass that WITHHELD ops did not converge the sink. The projection's
/// FK-grounding gates deliberately drop ops whose parent row would not exist at
/// COMMIT — emitting them would roll the whole batch back — but the dropped
/// change is still owed to SQL, and only another pass can pay it. `Ok(())`
/// cannot express that, and a caller that reads `Ok` as "the rows are written"
/// is then wrong without any error to show for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionPass {
    /// Every op the diff produced reached the sink.
    Converged,
    /// `withheld` op(s) were dropped as FK-ungrounded. The sink is behind the
    /// consolidator until a later pass re-emits them against a grounded base.
    Incomplete { withheld: usize },
}

impl ProjectionPass {
    /// The op count this pass still owes the sink; zero when it converged.
    pub fn withheld(self) -> usize {
        match self {
            Self::Converged => 0,
            Self::Incomplete { withheld } => withheld,
        }
    }
}

/// Pull edge of the consolidator → sink convergent feed.
#[async_trait]
pub trait DownstreamProjection: Send + Sync {
    /// Project the consolidator's changes since its last watermark to the SQL
    /// sink, synchronously. Idempotent: a no-op when the watermark already
    /// matches the current consolidator state.
    ///
    /// Returns what the pass achieved. A caller whose own contract is "the
    /// rows are written when this returns" must re-drive or fail on
    /// [`ProjectionPass::Incomplete`] — it is not an error, but it is not
    /// success either.
    async fn flush(&self) -> Result<ProjectionPass>;
}
