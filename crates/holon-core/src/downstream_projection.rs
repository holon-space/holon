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

/// Pull edge of the consolidator → sink convergent feed.
#[async_trait]
pub trait DownstreamProjection: Send + Sync {
    /// Project the consolidator's changes since its last watermark to the SQL
    /// sink, synchronously. Idempotent: a no-op when the watermark already
    /// matches the current consolidator state.
    async fn flush(&self) -> Result<()>;
}
