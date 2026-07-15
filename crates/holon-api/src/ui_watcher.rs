//! The UI-render/watch capability trait (ADR 0004 — "Turso is one of four").
//!
//! The Turso implementation (CDC-driven `watch_ui` machinery) lives in
//! `holon::api::ui_watcher`; a no-Turso session renders via
//! `holon::api::loro_ui_watcher` from a `BlockQuerySource` snapshot instead.

use std::sync::Arc;

use anyhow::Result;

use crate::streaming::WatchHandle;
use crate::EntityUri;

/// The UI-render/watch capability.
///
/// Produces a long-lived `UiEvent` stream (structure + data) for a block. The
/// frontend holds this as `Arc<dyn UiWatcher>`; which pipeline backs it
/// (Turso CDC vs Loro snapshot) is a wiring decision.
#[async_trait::async_trait]
pub trait UiWatcher: Send + Sync {
    /// Watch a block's UI, returning a stream of UiEvents and a command
    /// channel.
    async fn watch_ui(self: Arc<Self>, block_id: EntityUri) -> Result<WatchHandle>;
}
