//! The read side of the downstream block sink.
//!
//! Only the *port* lives here — the same shape as [`crate::BoundaryEnforcer`].
//! The Loro→SQL projection that consumes it lives in `holon-loro`, and the
//! production Turso implementation lives in `holon`, so neither crate has to
//! know the other.

use std::collections::HashMap;

use anyhow::Result;
use holon_api::SnapshotBlock;

/// Read side of the downstream sink, used by the Loro projection as the diff
/// "before". Abstracts the concrete sink so the production Turso path and the
/// in-memory PBT stub share one projection. Returns the current persisted block
/// state keyed by stable id.
#[async_trait::async_trait]
pub trait SinkReader: Send + Sync {
    async fn read_blocks(&self) -> Result<HashMap<String, SnapshotBlock>>;
}
