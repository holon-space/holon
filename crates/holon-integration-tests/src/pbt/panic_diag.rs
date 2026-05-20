//! Engine-vs-GPUI panic-time diagnostic.
//!
//! When `inv-displayed-text` collects mismatches between the GPUI-rendered
//! string and the reference model's expected string, this helper asks the
//! shared `ReactiveEngine` what *it* thinks the widget should render. The
//! resulting tag tells you whether to look at the backend or the GPUI render
//! layer:
//!
//! - `EngineAlsoStale` — engine produces the same wrong text. Bug is upstream
//!   of GPUI (ReactiveViewModel / matview / CDC / SQL).
//! - `GpuiRenderOnly`  — engine produces the expected text. GPUI render layer
//!   failed to apply the engine's update (text widget missed its data signal,
//!   InputState skipped a refresh, etc.).
//! - `EngineThirdValue` — engine produces a third string, neither the on-screen
//!   nor the expected one. Rare; flagged separately so it doesn't get lumped
//!   into either bucket.
//! - `NoEngineWidget` — engine snapshot has no matching `editable_text` /
//!   `text` widget for the block (e.g. a sidebar `text(col(...))` whose
//!   widget is rendered by the *parent's* render expression, not the block's
//!   own). Diagnostic doesn't apply.
//!
//! The diagnostic is intentionally read-only and non-blocking: it runs once
//! while `inv-displayed-text` is building its panic message, then proptest
//! shrinks the failing case as before.

use std::sync::Arc;

use holon_api::EntityUri;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::view_model::{ViewKind, ViewModel};

/// Result of comparing one (block, on_screen, expected) triple to the
/// engine's snapshot.
#[derive(Debug, Clone)]
pub enum DiagTag {
    EngineAlsoStale,
    GpuiRenderOnly,
    EngineThirdValue(String),
    NoEngineWidget,
}

impl DiagTag {
    /// Short human-readable form for embedding in a panic message.
    pub fn as_label(&self) -> String {
        match self {
            DiagTag::EngineAlsoStale => {
                "engine snapshot also stale → backend (ViewModel/matview/CDC)".into()
            }
            DiagTag::GpuiRenderOnly => {
                "engine snapshot matches expected → GPUI render layer".into()
            }
            DiagTag::EngineThirdValue(v) => {
                format!("engine snapshot has third value {v:?} → investigate engine independently")
            }
            DiagTag::NoEngineWidget => {
                "no matching engine widget for this block (sidebar text? wrong scheme?)".into()
            }
        }
    }
}

/// Inspect the engine's snapshot for `block_id` and classify the mismatch.
pub fn diagnose_displayed_text(
    engine: &Arc<ReactiveEngine>,
    block_id: &str,
    on_screen: &str,
    expected: &str,
) -> DiagTag {
    let Ok(uri) = EntityUri::parse(block_id) else {
        return DiagTag::NoEngineWidget;
    };
    // `snapshot_resolved` (BuilderServices) recursively resolves nested
    // LiveBlocks — needed when the block's editable_text is inside a tree
    // / outline / table-row produced by an ancestor's render expression.
    let snapshot = engine.snapshot_resolved(&uri);
    let Some(content) = first_text_for_block(&snapshot, block_id) else {
        return DiagTag::NoEngineWidget;
    };
    if content == on_screen {
        DiagTag::EngineAlsoStale
    } else if content == expected {
        DiagTag::GpuiRenderOnly
    } else {
        DiagTag::EngineThirdValue(content)
    }
}

/// Walk the engine's ViewModel snapshot and return the first
/// `EditableText` or `Text` content whose entity id matches `target_id`.
///
/// Mirrors how GPUI's geometry registry attaches `entity_id` to text-bearing
/// widgets — comparing the engine's leaf content to the on-screen string is
/// the same comparison `inv-displayed-text` does, just at the engine level.
fn first_text_for_block(vm: &ViewModel, target_id: &str) -> Option<String> {
    if vm.entity_id().as_ref().map(|u| u.as_str()) == Some(target_id) {
        match &vm.kind {
            ViewKind::EditableText { content, .. } => return Some(content.clone()),
            ViewKind::Text { content, .. } => return Some(content.clone()),
            _ => {}
        }
    }
    for child in vm.children() {
        if let Some(c) = first_text_for_block(child, target_id) {
            return Some(c);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Render-tree + focus-history dumps for panic messages.
// ---------------------------------------------------------------------------
//
// When an `apply_X` panic fires (`wait_for_focus_to_match`, `wait_for_blocks_synced`,
// SplitBlock count mismatch, etc.), we want one consolidated message that tells you:
//   - what the GPUI render tree looks like *right now* (which entities are mounted,
//     under which widget type),
//   - what `UiState.focused_block` currently reports,
//   - the recent focus-change history per region (`navigation_history` rows).
//
// This eliminates the "I had to read an unrelated invariant's tree dump 200 lines
// earlier to reconstruct what failed at the panic site" pattern.

use holon::api::BackendEngine;
use holon_frontend::geometry::{ElementInfo, GeometryProvider};

/// Format the BoundsRegistry as a parent-indented tree. Each line ends with
/// the element's `widget_type`, `entity_id`, bounds and `has_content` flag —
/// the same shape `inv-frontend-engine TREE` already emits, just as a String
/// instead of `eprintln!`'d directly.
pub fn format_render_tree(elements: &[(String, ElementInfo)], label: &str) -> String {
    use std::collections::HashMap;
    use std::fmt::Write;

    if elements.is_empty() {
        return format!("[{label}] <empty render tree — BoundsRegistry has no entries>\n");
    }

    let by_id: HashMap<&str, &ElementInfo> =
        elements.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let mut children_of: HashMap<Option<&str>, Vec<&str>> = HashMap::new();
    let mut orphans: Vec<&str> = Vec::new();
    for (el_id, info) in elements {
        match info.parent_id.as_deref() {
            None => children_of.entry(None).or_default().push(el_id.as_str()),
            Some(p) if by_id.contains_key(p) => {
                children_of.entry(Some(p)).or_default().push(el_id.as_str())
            }
            Some(_) => orphans.push(el_id.as_str()),
        }
    }
    let sort_children = |ids: &mut Vec<&str>| {
        ids.sort_by(|a, b| {
            let ai = by_id[a];
            let bi = by_id[b];
            ai.y.partial_cmp(&bi.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(ai.x.partial_cmp(&bi.x).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.cmp(b))
        });
    };
    for ids in children_of.values_mut() {
        sort_children(ids);
    }
    sort_children(&mut orphans);

    let mut out = String::new();
    fn print_node(
        out: &mut String,
        id: &str,
        depth: usize,
        by_id: &HashMap<&str, &ElementInfo>,
        children_of: &HashMap<Option<&str>, Vec<&str>>,
        label: &str,
    ) {
        let info = by_id[id];
        let indent = "  ".repeat(depth);
        let _ = writeln!(
            out,
            "[{label}] {indent}{id}: widget_type={} entity_id={:?} bounds=({:.0},{:.0} {:.0}x{:.0}) has_content={}",
            info.widget_type,
            info.entity_id,
            info.x,
            info.y,
            info.width,
            info.height,
            info.has_content,
        );
        if let Some(kids) = children_of.get(&Some(id)) {
            for child in kids {
                print_node(out, child, depth + 1, by_id, children_of, label);
            }
        }
    }
    if let Some(roots) = children_of.get(&None) {
        for root in roots {
            print_node(&mut out, root, 0, &by_id, &children_of, label);
        }
    }
    if !orphans.is_empty() {
        let _ = writeln!(
            out,
            "[{label}] <orphan> ({} entries — parent_id refers to missing element)",
            orphans.len()
        );
        for id in &orphans {
            print_node(&mut out, id, 1, &by_id, &children_of, label);
        }
    }
    out
}

/// One row from `navigation_history` formatted for human reading.
#[derive(Debug, Clone)]
pub struct NavHistoryRow {
    pub id: i64,
    pub region: String,
    pub block_id: Option<String>,
    pub timestamp: Option<String>,
    pub closed_at: Option<String>,
}

/// Query the last `limit` rows of `navigation_history`, ordered newest first.
///
/// Read-only — never writes, never blocks long. Errors are folded into a
/// single user-visible row so panic dumps still produce something useful even
/// if the table doesn't exist yet (e.g. very early in startup).
pub async fn fetch_recent_nav_history(engine: &BackendEngine, limit: i64) -> Vec<NavHistoryRow> {
    use holon_api::Value;
    use std::collections::HashMap;

    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("limit".to_string(), Value::Integer(limit));
    let sql = "SELECT id, region, block_id, timestamp, closed_at \
               FROM navigation_history ORDER BY id DESC LIMIT $limit";
    let rows = match engine.db_handle().query(sql, params).await {
        Ok(rows) => rows,
        Err(e) => {
            return vec![NavHistoryRow {
                id: -1,
                region: format!("<query failed: {e}>"),
                block_id: None,
                timestamp: None,
                closed_at: None,
            }];
        }
    };
    rows.into_iter()
        .map(|row| NavHistoryRow {
            id: row.get("id").and_then(|v| v.as_i64()).unwrap_or(-1),
            region: row
                .get("region")
                .and_then(|v| v.as_string())
                .map(String::from)
                .unwrap_or_else(|| "<missing>".into()),
            block_id: row
                .get("block_id")
                .and_then(|v| v.as_string())
                .map(String::from),
            timestamp: row
                .get("timestamp")
                .and_then(|v| v.as_string())
                .map(String::from),
            closed_at: row
                .get("closed_at")
                .and_then(|v| v.as_string())
                .map(String::from),
        })
        .collect()
}

/// Format the navigation-history rows for inclusion in a panic message.
/// Newest first; closed rows are tagged. Empty result is reported plainly.
pub fn format_nav_history(rows: &[NavHistoryRow], label: &str) -> String {
    use std::fmt::Write;

    if rows.is_empty() {
        return format!("[{label}] <navigation_history empty>\n");
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "[{label}] {} most-recent focus-change row(s) (newest first):",
        rows.len()
    );
    for r in rows {
        let block = r.block_id.as_deref().unwrap_or("<null>");
        let ts = r.timestamp.as_deref().unwrap_or("<no-ts>");
        let closed = r
            .closed_at
            .as_deref()
            .map(|c| format!(" closed_at={c}"))
            .unwrap_or_else(|| " OPEN".to_string());
        let _ = writeln!(
            out,
            "[{label}]   id={} region={} block_id={} ts={}{}",
            r.id, r.region, block, ts, closed,
        );
    }
    out
}

/// One-stop panic-diagnostic dump: focused_block, navigation_history, render tree.
///
/// All async because `navigation_history` is queried via the engine's
/// `DbHandle`. The result is intended to be appended to a `panic!` message,
/// so it's just a `String`.
pub async fn focus_and_render_dump(
    engine: &BackendEngine,
    ui_state_focused_block: Option<&holon_api::EntityUri>,
    geometry: Option<&dyn GeometryProvider>,
    label: &str,
) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let _ = writeln!(out, "\n=== focus & render diagnostic ({label}) ===");
    let _ = writeln!(
        out,
        "[{label}] UiState.focused_block = {:?}",
        ui_state_focused_block.map(|u| u.to_string())
    );
    let rows = fetch_recent_nav_history(engine, 8).await;
    out.push_str(&format_nav_history(&rows, label));
    if let Some(geom) = geometry {
        let elements = geom.all_elements();
        out.push_str(&format_render_tree(&elements, label));
    } else {
        let _ = writeln!(
            out,
            "[{label}] <no geometry provider — render tree unavailable>"
        );
    }
    out
}
