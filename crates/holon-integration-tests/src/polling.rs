//! Polling utilities for waiting on async conditions

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use holon::api::{RowChange, RowChangeStream};
use holon::testing::e2e_test_helpers::E2ETestContext;
use holon_api::{QueryLanguage, Value};
use holon_frontend::geometry::{ElementInfo, GeometryProvider};
use tokio_stream::StreamExt;

use crate::widget_state::{WidgetLocator, WidgetStateModel};

/// Wait until a condition is met or timeout expires.
/// Returns true if condition was met, false if timed out.
pub async fn wait_until<F, Fut>(predicate: F, timeout: Duration, poll_interval: Duration) -> bool
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if predicate().await {
            return true;
        }
        tokio::time::sleep(poll_interval).await;
    }
    false
}

/// Wait until expected block count is reached in a query result.
pub async fn wait_for_block_count(
    ctx: &E2ETestContext,
    prql: &str,
    expected_count: usize,
    timeout: Duration,
) -> Vec<HashMap<String, Value>> {
    let poll_interval = Duration::from_millis(10);
    let start = Instant::now();
    let mut last_result = Vec::new();

    while start.elapsed() < timeout {
        if let Ok(rows) = ctx
            .query(prql.to_string(), QueryLanguage::HolonPrql, HashMap::new())
            .await
        {
            if rows.len() == expected_count {
                return rows;
            }
            last_result = rows;
        }
        tokio::time::sleep(poll_interval).await;
    }
    last_result
}

/// Wait until a specific block exists in the database.
pub async fn wait_for_block(ctx: &E2ETestContext, block_id: &str, timeout: Duration) -> bool {
    let sql = format!("SELECT id FROM block WHERE id = '{}'", block_id);
    let poll_interval = Duration::from_millis(50);

    wait_until(
        || async {
            ctx.query(sql.clone(), QueryLanguage::HolonSql, HashMap::new())
                .await
                .map(|rows| !rows.is_empty())
                .unwrap_or(false)
        },
        timeout,
        poll_interval,
    )
    .await
}

/// Wait until file content matches a condition.
pub async fn wait_for_file_condition<F>(file_path: &Path, condition: F, timeout: Duration) -> bool
where
    F: Fn(&str) -> bool,
{
    let poll_interval = Duration::from_millis(10);
    let start = Instant::now();

    while start.elapsed() < timeout {
        if let Ok(content) = tokio::fs::read_to_string(file_path).await
            && condition(&content)
        {
            return true;
        }
        tokio::time::sleep(poll_interval).await;
    }
    false
}

/// Drain all pending events from a CDC stream without blocking.
///
/// Returns all events that were available immediately. Use this to
/// process any pending changes before making assertions.
pub async fn drain_stream(stream: &mut RowChangeStream) -> Vec<RowChange> {
    let mut changes = Vec::new();
    let drain_timeout = Duration::from_millis(10);

    loop {
        match tokio::time::timeout(drain_timeout, stream.next()).await {
            Ok(Some(batch)) => {
                changes.extend(batch.inner.items);
            }
            _ => break,
        }
    }

    changes
}

/// Wait until `BoundsRegistry` has committed bounds for the given block
/// entity. `GpuiUserDriver` fails loud when bounds aren't available —
/// call this after any transition that creates or re-parents a block,
/// before driving user input against it.
///
/// The `BoundsRegistry` double-buffers staged → committed per render
/// pass, so a just-added element stays invisible to the driver until the
/// next `begin_pass`. This polls the registry with a short interval and
/// returns the committed `ElementInfo` or an error on timeout.
pub async fn wait_for_element_bounds(
    geometry: &dyn GeometryProvider,
    entity_id: &str,
    timeout: Duration,
) -> Result<ElementInfo> {
    let el_id = format!("render-entity-{entity_id}");
    let poll_interval = Duration::from_millis(5);
    let deadline = Instant::now() + timeout;

    loop {
        if let Some(info) = geometry.element_info(&el_id) {
            return Ok(info);
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "wait_for_element_bounds: timed out after {timeout:?} waiting for \
                 bounds of entity {entity_id:?} (element id {el_id:?}) — element \
                 was never rendered, or BoundsRegistry never promoted staged → \
                 committed. Check that the element actually entered the render \
                 tree after the triggering mutation."
            ))
            .with_context(|| format!("entity {entity_id}"));
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// Wait until a widget matching the locator contains the expected text.
///
/// Drains CDC events and applies them to the state model until the text
/// is found or timeout expires.
///
/// Returns true if text was found, false if timed out.
pub async fn wait_for_text_in_widget(
    stream: &mut RowChangeStream,
    state: &mut WidgetStateModel,
    locator: &WidgetLocator,
    expected_text: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;

    // First check current state
    if state.contains_text(locator, expected_text) {
        return true;
    }

    // Then wait for changes
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(batch)) => {
                for change in batch.inner.items {
                    state.apply_change(&change);
                }
                if state.contains_text(locator, expected_text) {
                    return true;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    false
}
