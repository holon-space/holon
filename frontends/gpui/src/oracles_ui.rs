//! Live-oracle UI wiring: banner + status bridge + engine snapshot access.
//!
//! Debug builds run a subset of the keystone PBT invariants as background
//! oracles (see `holon-oracles`). This module contains the GPUI-specific
//! pieces:
//!
//! - [`EngineOracleAccess`] — `OracleStateAccess` over `BackendEngine` (the
//!   same concurrency-safe SQL read path the embedded MCP server uses).
//! - [`spawn_oracle_runner`] — starts the background cheap-tier loop on the
//!   tokio runtime (called from `main.rs`, off the GPUI thread).
//! - [`spawn_oracle_bridge`] — global-status → GPUI-entity pump, mirroring
//!   `share_ui::spawn_degraded_bus_bridge`.
//! - [`render_banner`] — the impossible-to-miss red top banner (fail loud,
//!   never fake).

use std::sync::Arc;

use gpui::AnyElement;
use gpui::AnyWindowHandle;
use gpui::AsyncApp;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::IntoElement;
use gpui::MouseButton;
use gpui::SharedString;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use holon::api::backend_engine::BackendEngine;
use holon::storage::BLOCK_READ_TABLE;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityUri;
use holon_oracles::OracleMode;
use holon_oracles::checks::ParentRow;
use holon_oracles::checks::SourceLanguageRow;
use holon_oracles::runner::OracleRunnerConfig;
use holon_oracles::runner::OracleStateAccess;
use holon_oracles::runner::run_oracle_loop;
use holon_oracles::status::OracleStatus;
use holon_oracles::status::Violation;

// ─── Engine snapshot access ─────────────────────────────────────────────────

pub struct EngineOracleAccess {
    engine: Arc<BackendEngine>,
}

fn parse_uri(row: &holon_api::StorageEntity, column: &str) -> anyhow::Result<EntityUri> {
    // A NULL/missing `parent_id` is a top-level block — semantically
    // `no_parent()` (same coalescing as the keystone's `parse_block_row`).
    match row.get(column).and_then(|v| v.as_string()) {
        Some(s) => EntityUri::parse(s)
            .map_err(|e| anyhow::anyhow!("oracle row column '{column}' is not a URI: {e}")),
        None if column == "parent_id" => Ok(EntityUri::no_parent()),
        None => anyhow::bail!("oracle row missing column '{column}': {row:?}"),
    }
}

impl EngineOracleAccess {
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self { engine }
    }

    async fn parent_rows_from(&self, table: &str) -> anyhow::Result<Vec<ParentRow>> {
        let sql = format!("SELECT id, parent_id FROM {table}");
        let rows = self
            .engine
            .execute_query(sql, std::collections::HashMap::new(), None)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(ParentRow {
                    id: parse_uri(r, "id")?,
                    parent_id: parse_uri(r, "parent_id")?,
                })
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl OracleStateAccess for EngineOracleAccess {
    async fn matview_parent_rows(&self) -> anyhow::Result<Vec<ParentRow>> {
        self.parent_rows_from(BLOCK_READ_TABLE).await
    }

    async fn raw_parent_rows(&self) -> anyhow::Result<Vec<ParentRow>> {
        self.parent_rows_from(BLOCK_WRITE_TABLE).await
    }

    async fn source_language_rows(&self) -> anyhow::Result<Vec<SourceLanguageRow>> {
        let sql = format!("SELECT id, content_type, source_language FROM {BLOCK_WRITE_TABLE}");
        let rows = self
            .engine
            .execute_query(sql, std::collections::HashMap::new(), None)
            .await?;
        rows.iter()
            .map(|r| {
                let content_type = r
                    .get("content_type")
                    .and_then(|v| v.as_string())
                    .unwrap_or("text")
                    .to_string();
                Ok(SourceLanguageRow {
                    id: parse_uri(r, "id")?,
                    is_source: content_type == "source",
                    source_language: r
                        .get("source_language")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string()),
                })
            })
            .collect()
    }
}

/// Start the background cheap-tier oracle loop. Call from `main.rs` after
/// bootstrap, under `cfg(debug_assertions)`.
pub fn spawn_oracle_runner(
    engine: Arc<BackendEngine>,
    rt_handle: &tokio::runtime::Handle,
    mode: OracleMode,
) {
    let access: Arc<dyn OracleStateAccess> = Arc::new(EngineOracleAccess::new(engine));
    rt_handle.spawn(run_oracle_loop(access, mode, OracleRunnerConfig::default()));
}

// ─── GPUI status entity + bridge ────────────────────────────────────────────

/// Per-window mirror of the global [`OracleStatus`] snapshot.
#[derive(Default)]
pub struct OracleUiState {
    pub violations: Vec<Violation>,
}

pub struct NotifyOracleUi;
impl EventEmitter<NotifyOracleUi> for OracleUiState {}

/// Spawn the global-status → GPUI-entity pump (mirrors
/// `share_ui::spawn_degraded_bus_bridge`): a tokio task awaits the status
/// watch channel and forwards snapshots through an unbounded mpsc to a pump
/// on GPUI's executor, which mutates the entity and notifies.
pub fn spawn_oracle_bridge(
    rt_handle: &tokio::runtime::Handle,
    oracle_state: Entity<OracleUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Vec<Violation>>();

    rt_handle.spawn(async move {
        let mut watch = OracleStatus::global().watch();
        loop {
            if watch.changed().await.is_err() {
                return; // status dropped (process exit)
            }
            if tx
                .unbounded_send(OracleStatus::global().snapshot())
                .is_err()
            {
                return; // pump gone
            }
        }
    });

    async_cx
        .spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(violations) = rx.next().await {
                let updated = cx.update_window(window_handle, |_, window, cx| {
                    oracle_state.update(cx, |s, cx| {
                        s.violations = violations.clone();
                        cx.emit(NotifyOracleUi);
                        cx.notify();
                    });
                    window.refresh();
                });
                if let Err(e) = updated {
                    // Fail loud: a broken bridge means violations are
                    // invisible in the UI channel — never swallow that.
                    tracing::error!(
                        target: "holon_oracles",
                        "oracle UI bridge failed to update window ({} violations pending): {e}",
                        violations.len()
                    );
                }
            }
        })
        .detach();
}

// ─── Banner ─────────────────────────────────────────────────────────────────

/// Render the oracle-violation banner: a full-width red bar pinned to the top
/// of the window. Returns `None` when there is nothing to report.
pub fn render_banner(
    state: &OracleUiState,
    oracle_state: Entity<OracleUiState>,
) -> Option<AnyElement> {
    if state.violations.is_empty() {
        return None;
    }

    const MAX_LINES: usize = 4;
    let total = state.violations.len();
    let mut lines = div().flex().flex_col().gap_1();
    for v in state.violations.iter().take(MAX_LINES) {
        // Messages are already self-tagged with their oracle id.
        lines = lines.child(div().text_size(px(12.0)).child(v.message.clone()));
    }
    if total > MAX_LINES {
        lines = lines.child(
            div()
                .text_size(px(12.0))
                .child(format!("… and {} more (see log)", total - MAX_LINES)),
        );
    }

    let banner = div()
        .id(SharedString::from("oracle-banner"))
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .px_3()
        .py_2()
        .bg(gpui::rgba(0xb91c1cee))
        .border_b_2()
        .border_color(gpui::rgba(0x7f1d1dff))
        .text_color(gpui::rgba(0xffffffff))
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(format!(
                            "ORACLE VIOLATION{} ({total}) — live invariant check failed",
                            if total == 1 { "" } else { "S" }
                        )),
                )
                .child(lines),
        )
        .child(
            div()
                .id(SharedString::from("oracle-banner-dismiss"))
                .cursor_pointer()
                .pl_3()
                .text_size(px(12.0))
                .child("dismiss ✕")
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    // Latency violations are sticky → clear them. Structural
                    // violations re-appear on the next runner cycle if the
                    // data is still broken (loud, not silenceable).
                    OracleStatus::global().dismiss_latency();
                    oracle_state.update(cx, |s, cx| {
                        s.violations.clear();
                        cx.emit(NotifyOracleUi);
                        cx.notify();
                    });
                }),
        );

    Some(banner.into_any_element())
}
