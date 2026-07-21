//! Open-blocks tab strip for the MAIN region (Increment 1 + 2 of
//! "tabs for open blocks").
//!
//! The engine (Increment 0) keeps ALL open main tabs in the `focus_roots`
//! matview (one row per open tab: `region`, `root_id`, `added_ts`,
//! `history_id`) and the active tab's `history_id` in `navigation_cursor`.
//! The main panel already renders ONLY the active tab (cursor-filtered). This
//! module renders the horizontal strip of open tabs above the breadcrumb bar,
//! highlights the active one, and switches tabs via the `navigation.activate`
//! op (move the region's cursor to an already-open history row — no reorder,
//! no scroll reset).
//!
//! Async resolution mirrors `breadcrumb::resolve_breadcrumb`: `HolonApp::
//! render` detects a focus change and calls [`resolve_tab_strip`], which runs
//! the two reads on tokio and pumps the tabs back into this entity, emitting
//! [`NotifyTabStrip`] to re-render.
//!
//! Fail-loud: a strip that can't be resolved lands in `error` and is rendered
//! as a visible message, never a silently-empty bar.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::AnyWindowHandle;
use gpui::AsyncApp;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::MouseButton;
use gpui::SharedString;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;

use crate::search_ui::SearchTheme;

/// One open MAIN-region tab.
#[derive(Clone, Debug)]
pub struct TabEntry {
    /// Row identity in `focus_roots` / `navigation_cursor` — what
    /// `navigation.activate` targets. NOT the block id.
    pub history_id: i64,
    /// The open block this tab shows.
    pub block_id: EntityUri,
    /// Tab caption (block content's first line).
    pub label: String,
}

/// Open-tabs strip state. Its own `Entity` so async resolution can update it
/// and trigger a re-render (same pattern as `BreadcrumbState`).
pub struct TabStripState {
    /// Open main tabs in stable insertion order (ORDER BY history_id, Q3 —
    /// never `added_ts`, never move-to-top).
    pub tabs: Vec<TabEntry>,
    /// The active tab's `history_id` (the region cursor). `None` until
    /// resolved.
    pub active_history_id: Option<i64>,
    pub error: Option<String>,
    /// Drops stale async responses when the strip is re-resolved rapidly.
    pub generation: u64,
}

impl Default for TabStripState {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active_history_id: None,
            error: None,
            generation: 0,
        }
    }
}

pub struct NotifyTabStrip;
impl EventEmitter<NotifyTabStrip> for TabStripState {}

/// Resolved payload pumped back from the async read.
struct ResolvedStrip {
    tabs: Vec<TabEntry>,
    active_history_id: Option<i64>,
}

const OPEN_TABS_SQL: &str = "SELECT fr.history_id, fr.root_id, b.content \
     FROM focus_roots fr JOIN block b ON b.id = fr.root_id \
     WHERE fr.region = 'main' ORDER BY fr.history_id";

const CURSOR_SQL: &str = "SELECT history_id FROM navigation_cursor WHERE region = 'main'";

fn require_i64(row: &HashMap<String, Value>, col: &str) -> Result<i64, String> {
    match row.get(col) {
        Some(Value::Integer(i)) => Ok(*i),
        other => Err(format!(
            "tab strip: column {col:?} expected Integer, got {other:?}"
        )),
    }
}

fn require_string(row: &HashMap<String, Value>, col: &str) -> Result<String, String> {
    match row.get(col) {
        Some(Value::String(s)) => Ok(s.clone()),
        other => Err(format!(
            "tab strip: column {col:?} expected String, got {other:?}"
        )),
    }
}

/// Resolve the open MAIN tabs + active cursor and pump them back into `state`.
/// Region-global (no block id): reads `focus_roots` + `navigation_cursor`.
pub fn resolve_tab_strip(
    generation: u64,
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    state: Entity<TabStripState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) {
    let (tx, rx) = futures::channel::oneshot::channel::<Result<ResolvedStrip, String>>();
    rt_handle.spawn(async move {
        let outcome = resolve_strip_query(&session).await;
        let _ = tx.send(outcome);
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                state.update(cx, |s, cx| {
                    if s.generation == generation {
                        match outcome {
                            Ok(Ok(resolved)) => {
                                s.tabs = resolved.tabs;
                                s.active_history_id = resolved.active_history_id;
                                s.error = None;
                            }
                            Ok(Err(e)) => {
                                s.tabs.clear();
                                s.active_history_id = None;
                                s.error = Some(e);
                            }
                            Err(_cancelled) => {
                                s.error = Some("tab strip task dropped".to_string());
                            }
                        }
                        cx.emit(NotifyTabStrip);
                        cx.notify();
                    }
                });
            });
        })
        .detach();
}

async fn resolve_strip_query(session: &Arc<FrontendSession>) -> Result<ResolvedStrip, String> {
    let qe = session
        .query_engine()
        .ok_or_else(|| "tab strip needs the Turso query backend".to_string())?;

    let tab_rows = qe
        .execute_query(OPEN_TABS_SQL, QueryLanguage::HolonSql, HashMap::new(), None)
        .await
        .map_err(|e| format!("open-tabs query failed: {e:#}"))?;

    let mut tabs = Vec::with_capacity(tab_rows.len());
    for row in &tab_rows {
        let history_id = require_i64(row, "history_id")?;
        let root_id = require_string(row, "root_id")?;
        let label = require_string(row, "content")?;
        tabs.push(TabEntry {
            history_id,
            // ALLOW(entity_uri_from_raw): focus_roots matview row root_id (schemed block id)
            block_id: EntityUri::from_raw(&root_id),
            label,
        });
    }

    let cursor_rows = qe
        .execute_query(CURSOR_SQL, QueryLanguage::HolonSql, HashMap::new(), None)
        .await
        .map_err(|e| format!("cursor query failed: {e:#}"))?;
    let active_history_id = match cursor_rows.first() {
        Some(row) => Some(require_i64(row, "history_id")?),
        None => None,
    };

    Ok(ResolvedStrip {
        tabs,
        active_history_id,
    })
}

fn tab_title(label: &str) -> String {
    let first = label.lines().next().unwrap_or("").trim();
    const MAX: usize = 24;
    if first.is_empty() {
        "Untitled".to_string()
    } else if first.chars().count() > MAX {
        let cut: String = first.chars().take(MAX).collect();
        format!("{cut}…")
    } else {
        first.to_string()
    }
}

/// The `navigation.activate` intent for a target tab (`history_id`): move the
/// main region's cursor to that already-open navigation-history row.
pub fn activate_intent(history_id: i64) -> OperationIntent {
    OperationIntent::new(
        "navigation".into(),
        "activate".to_string(),
        [
            ("region".to_string(), Value::String("main".to_string())),
            ("history_id".to_string(), Value::Integer(history_id)),
        ]
        .into_iter()
        .collect(),
    )
}

/// Index of the active tab within `tabs`, if the cursor points at an open tab.
fn active_index(tabs: &[TabEntry], active: Option<i64>) -> Option<usize> {
    let active = active?;
    tabs.iter().position(|t| t.history_id == active)
}

/// Target `history_id` when cycling by `delta` (+1 next, -1 prev) with
/// wrap-around. `None` when there are no tabs.
pub fn cycle_target(tabs: &[TabEntry], active: Option<i64>, delta: i64) -> Option<i64> {
    if tabs.is_empty() {
        return None;
    }
    let len = tabs.len() as i64;
    // Unknown active cursor → start from the first (next) / last (prev) tab.
    let cur = active_index(tabs, active).map(|i| i as i64).unwrap_or(0);
    let next = (cur + delta).rem_euclid(len) as usize;
    Some(tabs[next].history_id)
}

/// Target `history_id` for a 1-based jump to the Nth tab, or `None` when there
/// is no Nth tab.
pub fn jump_target(tabs: &[TabEntry], n: usize) -> Option<i64> {
    n.checked_sub(1)
        .and_then(|idx| tabs.get(idx))
        .map(|t| t.history_id)
}

/// Optimistically move the highlight to `history_id` and emit a re-render.
/// The content follows via the engine's CDC path (the `activate` op moved the
/// cursor); this only keeps the strip's own highlight in sync immediately,
/// since `activate` does NOT change the focused block and so does NOT trigger
/// the render-side focus-change re-resolve.
fn optimistic_activate(entity: &Entity<TabStripState>, history_id: i64, cx: &mut gpui::App) {
    entity.update(cx, |s, cx| {
        s.active_history_id = Some(history_id);
        cx.emit(NotifyTabStrip);
        cx.notify();
    });
}

/// Cycle the active main tab by `delta` (+1 next, -1 prev, wrapping): dispatch
/// `navigation.activate` and optimistically update the highlight. No-op when
/// there are no open tabs.
pub fn apply_cycle(
    entity: &Entity<TabStripState>,
    services: &Arc<dyn BuilderServices>,
    delta: i64,
    cx: &mut gpui::App,
) {
    let (tabs, active) = {
        let s = entity.read(cx);
        (s.tabs.clone(), s.active_history_id)
    };
    if let Some(target) = cycle_target(&tabs, active, delta) {
        services.dispatch_intent(activate_intent(target));
        optimistic_activate(entity, target, cx);
    }
}

/// Jump to the 1-based Nth open main tab: dispatch `navigation.activate` and
/// optimistically update the highlight. No-op when there is no Nth tab.
pub fn apply_jump(
    entity: &Entity<TabStripState>,
    services: &Arc<dyn BuilderServices>,
    n: usize,
    cx: &mut gpui::App,
) {
    let tabs = entity.read(cx).tabs.clone();
    if let Some(target) = jump_target(&tabs, n) {
        services.dispatch_intent(activate_intent(target));
        optimistic_activate(entity, target, cx);
    }
}

// TODO(declarative-strip): graduate to a declarative horizontal chrome-strip
// collection variant with active-tab styling bound to navigation_cursor
// (rules:/when: comparing history_id to the cursor) and a declarative close
// action passing col(history_id); blocked on a chrome-strip mount point
// outside the reactive panel tree. See docs plan
// tabs-open-blocks-2026-07-21.md §Declarative-first.
//
/// Render the open-tabs strip for the MAIN region. `None` when there are no
/// open main tabs (like the breadcrumb bar returns `None` when empty).
///
/// KNOWN RAW-STRIP LIMITATION: a plain `navigation.activate` (tab switch) does
/// NOT change the focused block, so the render-side focus-change trigger does
/// NOT re-resolve this strip on activate. The active-tab highlight is kept in
/// sync by the OPTIMISTIC update in the click handler here and in the keyboard
/// handlers (`apply_cycle` / `apply_jump`). A full re-resolve happens only when
/// the focused block changes (e.g. `navigation.open_tab`).
pub fn render_tab_strip(
    state_read: &TabStripState,
    state_entity: Entity<TabStripState>,
    services: Arc<dyn BuilderServices>,
    theme: SearchTheme,
    left_pad: f32,
) -> Option<gpui::AnyElement> {
    if let Some(err) = &state_read.error {
        return Some(
            div()
                .flex()
                .items_center()
                .h(px(28.0))
                .pl(px(left_pad))
                .pr(px(16.0))
                .border_b_1()
                .border_color(theme.border)
                .text_size(px(11.0))
                .text_color(gpui::rgb(0xd9534f))
                .child(format!("Tabs unavailable: {err}"))
                .into_any_element(),
        );
    }
    if state_read.tabs.is_empty() {
        return None;
    }

    let active = state_read.active_history_id;
    let mut bar = div()
        .id("tab-strip")
        .flex()
        .flex_row()
        .items_center()
        .flex_wrap()
        .h(px(30.0))
        .pl(px(left_pad))
        .pr(px(16.0))
        .gap(px(4.0))
        .border_b_1()
        .border_color(theme.border)
        .text_size(px(12.0))
        .text_color(theme.muted_fg);

    for tab in &state_read.tabs {
        let is_active = Some(tab.history_id) == active;
        let history_id = tab.history_id;
        let services = services.clone();
        let entity = state_entity.clone();
        let chip_fg = if is_active { theme.fg } else { theme.muted_fg };
        let mut chip = div()
            .id(SharedString::from(format!("tab-{history_id}")))
            .flex()
            .items_center()
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .text_color(chip_fg)
            .child(tab_title(&tab.label));
        if is_active {
            // Active tab: subtle filled background (the cursor row).
            chip = chip.bg(theme.selected_bg);
        } else {
            chip = chip
                .cursor_pointer()
                .hover(|s| s.bg(gpui::rgba(0xffffff14)))
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    services.dispatch_intent(activate_intent(history_id));
                    optimistic_activate(&entity, history_id, cx);
                });
        }
        bar = bar.child(chip);
    }

    Some(bar.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(ids: &[i64]) -> Vec<TabEntry> {
        ids.iter()
            .map(|&history_id| TabEntry {
                history_id,
                block_id: EntityUri::block(&format!("b{history_id}")),
                label: format!("Tab {history_id}"),
            })
            .collect()
    }

    #[test]
    fn cycle_next_wraps() {
        let t = tabs(&[10, 20, 30]);
        assert_eq!(cycle_target(&t, Some(10), 1), Some(20));
        assert_eq!(cycle_target(&t, Some(30), 1), Some(10));
    }

    #[test]
    fn cycle_prev_wraps() {
        let t = tabs(&[10, 20, 30]);
        assert_eq!(cycle_target(&t, Some(20), -1), Some(10));
        assert_eq!(cycle_target(&t, Some(10), -1), Some(30));
    }

    #[test]
    fn cycle_empty_is_none() {
        assert_eq!(cycle_target(&[], Some(1), 1), None);
    }

    #[test]
    fn jump_is_one_based_and_bounded() {
        let t = tabs(&[10, 20, 30]);
        assert_eq!(jump_target(&t, 1), Some(10));
        assert_eq!(jump_target(&t, 3), Some(30));
        assert_eq!(jump_target(&t, 4), None);
        assert_eq!(jump_target(&t, 0), None);
    }
}
