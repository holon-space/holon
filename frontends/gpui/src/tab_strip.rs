//! Open tabs for the MAIN region, browser-style: a count button in the title
//! row ([`render_tab_count_button`]) opening a list of the open tabs
//! ([`render_tab_list`]) with switch, per-tab close, and a new-tab action.
//!
//! An open main tab is an open `navigation_history` row (`closed_at IS NULL`)
//! and the active tab's `history_id` lives in `navigation_cursor`. A tab whose
//! `block_id` is NULL is a BLANK tab: it names no page and renders the region's
//! default view. Blank tabs are why this reads `navigation_history` rather than
//! the `focus_roots` matview, which drops NULL-block rows by design. The main
//! panel renders ONLY the active tab (cursor-filtered); switching moves the
//! cursor via `navigation.activate` — no reorder, no scroll reset.
//!
//! Async resolution mirrors `breadcrumb::resolve_breadcrumb`: `HolonApp::
//! render` re-resolves when the focused block or the main VIEW generation
//! changes and calls [`resolve_tab_strip`], which reads
//! `QueryEngine::region_open_tabs` on tokio and pumps the tabs back into this
//! entity, emitting [`NotifyTabStrip`] to re-render. The generation half is
//! what carries `activate` / `close` / `new_tab`, none of which move focus.
//!
//! Fail-loud: a resolution that fails lands in `error`, and the count button
//! then paints [`TAB_COUNT_ERROR_LABEL`] with the message in the list, never a
//! plausible-looking `▤ 0`.

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
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;

use crate::search_ui::SearchTheme;

/// Why the tab chrome has nothing trustworthy to show.
#[derive(Clone, Debug, PartialEq)]
pub enum TabError {
    /// The tabs could not be read. The next successful read replaces it.
    Read(String),
    /// A tab operation was refused. Survives later reads, because those succeed
    /// and would otherwise wipe it; cleared by the next write that lands.
    Write(String),
}

impl TabError {
    pub fn message(&self) -> &str {
        match self {
            TabError::Read(m) | TabError::Write(m) => m,
        }
    }
}

/// One open MAIN-region tab.
#[derive(Clone, Debug)]
pub struct TabEntry {
    /// Row identity in `navigation_history` / `navigation_cursor` — what
    /// `navigation.activate` targets. NOT the block id.
    pub history_id: i64,
    /// The open block this tab shows. `None` for a blank tab, which names no
    /// block and renders the region's default view.
    pub block_id: Option<EntityUri>,
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
    /// Why the chrome cannot show a trustworthy count. A failed READ is
    /// superseded by the next successful read; a refused WRITE is not — the
    /// read that follows it succeeds and would otherwise erase the refusal one
    /// frame later, leaving a confident count where an operation had just been
    /// rejected.
    pub error: Option<TabError>,
    /// Drops stale async responses when the strip is re-resolved rapidly.
    pub generation: u64,
    /// Whether the tab LIST popup is showing. Lives here rather than in an
    /// entity of its own so the list reads the same tabs the count button
    /// counts.
    pub list_open: bool,
    /// Tab writes dispatched from here that have not reported completion yet.
    ///
    /// A read taken while one is outstanding can return the pre-op world, so it
    /// is dropped rather than shown. The completing write asks for the re-read
    /// itself, which is why there is no retry budget here: the signal is the
    /// write finishing, not a guess about whether it has.
    writes_in_flight: u32,
    /// When the oldest outstanding write started. A write that never reports
    /// back would otherwise freeze the count at its pre-op value with nothing
    /// said; past [`SLOW_WRITE_DISCLOSE_AFTER`] the button shows it is waiting.
    oldest_write_started: Option<std::time::Instant>,
    /// Set when a write completes: the next render re-reads the tabs once.
    /// Cleared when it is honoured.
    pub needs_recheck: bool,
    /// Reads this chrome has issued. Only a counter for tests — the storm a
    /// frame-driven retry loop produces is invisible to any other assertion.
    pub reads_issued: u64,
}

impl TabStripState {
    /// Whether a write has been outstanding long enough that the count on
    /// screen should no longer pass for current.
    pub fn write_overdue(&self) -> bool {
        self.writes_in_flight > 0
            && self
                .oldest_write_started
                .is_some_and(|t| t.elapsed() >= SLOW_WRITE_DISCLOSE_AFTER)
    }
}

impl Default for TabStripState {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active_history_id: None,
            error: None,
            generation: 0,
            list_open: false,
            writes_in_flight: 0,
            oldest_write_started: None,
            needs_recheck: false,
            reads_issued: 0,
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

/// Resolve the open MAIN tabs + active cursor and pump them back into `state`.
/// Region-global (no block id).
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
                            // A write is still outstanding, so this read may
                            // predate it. Drop it; the write asks for a fresh
                            // one when it lands.
                            Ok(Ok(_)) if s.writes_in_flight > 0 => {}
                            Ok(Ok(resolved)) => {
                                s.tabs = resolved.tabs;
                                s.active_history_id = resolved.active_history_id;
                                // Only a READ error is answered by a good read.
                                if matches!(s.error, Some(TabError::Read(_))) {
                                    s.error = None;
                                }
                            }
                            Ok(Err(e)) => {
                                s.tabs.clear();
                                s.active_history_id = None;
                                s.error = Some(TabError::Read(e));
                            }
                            Err(_cancelled) => {
                                s.error =
                                    Some(TabError::Read("tab strip task dropped".to_string()));
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

    let resolved = qe
        .region_open_tabs(holon_api::Region::Main)
        .await
        .map_err(|e| format!("open-tabs read failed: {e:#}"))?;

    Ok(ResolvedStrip {
        tabs: resolved
            .tabs
            .into_iter()
            .map(|tab| TabEntry {
                history_id: tab.history_id,
                block_id: tab.block_id,
                label: tab.caption.unwrap_or_default(),
            })
            .collect(),
        active_history_id: resolved.active_history_id,
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

/// What a tab is called on screen. A blank tab names no block, so it is titled
/// by what it is rather than by an empty block's "Untitled".
fn tab_caption(tab: &TabEntry) -> String {
    match tab.block_id {
        None => "New tab".to_string(),
        Some(_) => tab_title(&tab.label),
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

/// The `navigation.close` intent for a tab (`history_id`): soft-close that open
/// navigation-history row. When it is the active tab, the engine follows the
/// cursor to a neighbor (left, then right) so the panel never goes blank.
pub fn close_intent(history_id: i64) -> OperationIntent {
    OperationIntent::new(
        "navigation".into(),
        "close".to_string(),
        [("history_id".to_string(), Value::Integer(history_id))]
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

/// Highlight the cursor should follow to after `closed_id` is removed,
/// mirroring the engine's cursor-follow (LEFT neighbor first, then RIGHT).
/// `None` when no tab remains. Pure, so it is unit-tested without a window.
pub fn neighbor_after_close(tabs: &[TabEntry], closed_id: i64) -> Option<i64> {
    let idx = tabs.iter().position(|t| t.history_id == closed_id)?;
    if idx > 0 {
        return Some(tabs[idx - 1].history_id);
    }
    tabs.get(idx + 1).map(|t| t.history_id)
}

/// Optimistically remove a closed tab from the strip and, if it was the active
/// tab, move the highlight to the neighbor the engine will follow to. The
/// content + a full re-resolve follow via the engine's CDC path.
fn optimistic_close(entity: &Entity<TabStripState>, history_id: i64, cx: &mut gpui::App) {
    entity.update(cx, |s, cx| {
        if s.active_history_id == Some(history_id) {
            s.active_history_id = neighbor_after_close(&s.tabs, history_id);
        }
        s.tabs.retain(|t| t.history_id != history_id);
        cx.emit(NotifyTabStrip);
        cx.notify();
    });
}

/// Dispatch a tab op and re-read the tabs when the WRITE COMPLETES.
///
/// The chrome's own latch re-reads on the generation the op bumps at DISPATCH,
/// which can beat the write to the database; that read is dropped (see
/// `writes_in_flight`) and this is what asks for the one that replaces it. A
/// failed op surfaces in the chrome rather than leaving the optimistic view
/// standing as if it had worked.
fn dispatch_tab_op(
    entity: &Entity<TabStripState>,
    services: &Arc<dyn BuilderServices>,
    intent: OperationIntent,
    cx: &mut gpui::App,
) {
    let op = format!("{}.{}", intent.entity_name, intent.op_name);
    entity.update(cx, |s, _| {
        s.writes_in_flight += 1;
        s.oldest_write_started
            .get_or_insert_with(std::time::Instant::now);
    });
    let settled = services.dispatch_intent_awaitable(intent);

    // Repaint once the write has been outstanding too long, so the button can
    // say so. Without this nothing wakes the window while a write hangs and the
    // count silently keeps its old value.
    {
        let entity = entity.clone();
        let op = op.clone();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(SLOW_WRITE_DISCLOSE_AFTER)
                .await;
            let _ = entity.update(cx, |s, cx| {
                if s.writes_in_flight > 0 {
                    tracing::warn!(
                        op = %op,
                        seconds = SLOW_WRITE_DISCLOSE_AFTER.as_secs(),
                        "tab write has not reported back; the tab count is showing the state from \
                         before it and says so"
                    );
                    cx.notify();
                }
            });
        })
        .detach();
    }

    let entity = entity.clone();
    cx.spawn(async move |cx| {
        let outcome = settled.await;
        let _ = entity.update(cx, |s, cx| {
            s.writes_in_flight = s.writes_in_flight.saturating_sub(1);
            if s.writes_in_flight == 0 {
                s.oldest_write_started = None;
            }
            match outcome {
                Err(e) => {
                    // No re-read: the next one would clear this error a frame
                    // later and paint a count as if nothing had gone wrong. The
                    // refusal stands until something else navigates.
                    tracing::error!(op = %op, error = %format!("{e:#}"), "tab operation refused");
                    s.error = Some(TabError::Write(format!("{op} failed: {e:#}")));
                }
                Ok(_) => {
                    if matches!(s.error, Some(TabError::Write(_))) {
                        s.error = None;
                    }
                    s.needs_recheck = true;
                }
            }
            cx.emit(NotifyTabStrip);
            cx.notify();
        });
    })
    .detach();
}

/// [`dispatch_tab_op`] for a windowed test that needs to drive a refusal.
#[cfg(feature = "pbt")]
pub fn dispatch_tab_op_for_test(
    entity: &Entity<TabStripState>,
    services: &Arc<dyn BuilderServices>,
    intent: OperationIntent,
    cx: &mut gpui::App,
) {
    dispatch_tab_op(entity, services, intent, cx);
}

/// Mark a tab write in flight that will never complete, so a windowed test can
/// see what the chrome shows while it waits. Nothing decrements this — it
/// freezes the count for the window's lifetime, which is why it exists only in
/// test builds.
#[cfg(feature = "pbt")]
pub fn begin_stuck_write_for_test(entity: &Entity<TabStripState>, cx: &mut gpui::App) {
    entity.update(cx, |s, cx| {
        s.writes_in_flight += 1;
        s.oldest_write_started = Some(std::time::Instant::now() - SLOW_WRITE_DISCLOSE_AFTER);
        tracing::warn!(
            op = "test.stuck_write",
            seconds = SLOW_WRITE_DISCLOSE_AFTER.as_secs(),
            "tab write has not reported back; the tab count is showing the state from before it \
             and says so"
        );
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
        optimistic_activate(entity, target, cx);
        dispatch_tab_op(entity, services, activate_intent(target), cx);
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
        optimistic_activate(entity, target, cx);
        dispatch_tab_op(entity, services, activate_intent(target), cx);
    }
}

/// The title-row button carrying the open-tab count. Its `displayed_text` is
/// the count, so a windowed test reads the number the user sees.
pub const TAB_COUNT_BUTTON_ID: &str = "chrome-tab-count";
/// The list's new-tab action.
pub const TAB_LIST_NEW_ID: &str = "tab-list-new";
/// The list's error panel — carries the resolution failure's message.
pub const TAB_LIST_ERROR_ID: &str = "tab-list-error";

pub fn tab_list_row_id(history_id: i64) -> String {
    format!("tab-list-row-{history_id}")
}

pub fn tab_list_close_id(history_id: i64) -> String {
    format!("tab-list-close-{history_id}")
}

/// The `navigation.new_tab` intent: open one more tab in the main region and
/// move the cursor to it.
pub fn new_tab_intent() -> OperationIntent {
    OperationIntent::new(
        "navigation".into(),
        "new_tab".into(),
        [("region".to_string(), Value::String("main".to_string()))]
            .into_iter()
            .collect(),
    )
}

fn set_list_open(entity: &Entity<TabStripState>, open: bool, cx: &mut gpui::App) {
    entity.update(cx, |s, cx| {
        s.list_open = open;
        cx.emit(NotifyTabStrip);
        cx.notify();
    });
}

/// What the count button paints when the tabs could not be resolved. A count is
/// unavailable, not zero — `▤ 0` would be a plausible-looking lie.
pub const TAB_COUNT_ERROR_LABEL: &str = "▤ !";
/// What it paints while a write has been outstanding too long: the number shown
/// is from before that write, and this says so rather than letting it pass as
/// current.
pub const TAB_COUNT_WAITING_LABEL: &str = "▤ …";
/// How long a write may be outstanding before the chrome discloses that what it
/// shows predates it.
pub const SLOW_WRITE_DISCLOSE_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

/// The title-row button: how many tabs are open, and the door to the list.
///
/// When resolution failed it paints [`TAB_COUNT_ERROR_LABEL`] in the error
/// colour instead of a number, and the list behind it carries the message.
pub fn render_tab_count_button(
    state_read: &TabStripState,
    state_entity: Entity<TabStripState>,
    bounds: crate::geometry::BoundsRegistry,
    theme: SearchTheme,
) -> gpui::AnyElement {
    let failed = state_read.error.is_some();
    let waiting = state_read.write_overdue();
    let label = if failed {
        TAB_COUNT_ERROR_LABEL.to_string()
    } else if waiting {
        TAB_COUNT_WAITING_LABEL.to_string()
    } else {
        format!("▤ {}", state_read.tabs.len())
    };
    let fg = if failed {
        gpui::rgb(0xd9534f).into()
    } else if waiting {
        gpui::rgb(0xd0a215).into()
    } else {
        theme.muted_fg
    };
    let was_open = state_read.list_open;
    crate::geometry::TransparentTracker::new(
        TAB_COUNT_BUTTON_ID.to_string(),
        "tab_count_button",
        bounds,
        div()
            .id("tab-count-button")
            .cursor_pointer()
            .text_size(px(13.0))
            .px(px(6.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .text_color(fg)
            .hover(|s| s.bg(gpui::rgba(0x00000010)))
            .child(label.clone())
            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                cx.stop_propagation();
                set_list_open(&state_entity, !was_open, cx);
            })
            .into_any_element(),
    )
    .with_displayed_text(label)
    .into_any_element()
}

/// The tab LIST the count button opens: one row per open tab (switch), a close
/// affordance per row, and the new-tab action. `None` while it is closed.
///
/// Switching and creating both dismiss the list — a popup left standing over a
/// page the user has navigated away from is stale chrome.
pub fn render_tab_list(
    state_read: &TabStripState,
    state_entity: Entity<TabStripState>,
    services: Arc<dyn BuilderServices>,
    bounds: crate::geometry::BoundsRegistry,
    theme: SearchTheme,
    anchor_top: f32,
) -> Option<gpui::AnyElement> {
    if !state_read.list_open {
        return None;
    }

    // A failed resolution has no tabs to list, and listing none of them would
    // read as "no tabs are open". Show why instead.
    if let Some(err) = &state_read.error {
        let message = format!("Tabs unavailable: {}", err.message());
        return Some(
            list_backdrop(state_entity)
                .child(
                    crate::geometry::TransparentTracker::new(
                        TAB_LIST_ERROR_ID.to_string(),
                        "tab_list_error",
                        bounds,
                        tab_list_panel(theme, anchor_top)
                            .text_color(gpui::rgb(0xd9534f))
                            .child(message.clone())
                            .into_any_element(),
                    )
                    .with_displayed_text(message),
                )
                .into_any_element(),
        );
    }

    let active = state_read.active_history_id;
    let mut panel = tab_list_panel(theme, anchor_top);

    for tab in &state_read.tabs {
        let history_id = tab.history_id;
        let is_active = Some(history_id) == active;
        let caption = tab_caption(tab);

        let close_services = services.clone();
        let close_entity = state_entity.clone();
        let close_btn = crate::geometry::TransparentTracker::new(
            tab_list_close_id(history_id),
            "tab_list_close",
            bounds.clone(),
            div()
                .id(SharedString::from(format!(
                    "tab-list-close-btn-{history_id}"
                )))
                .ml(px(8.0))
                .px(px(4.0))
                .rounded(px(3.0))
                .cursor_pointer()
                .text_color(theme.muted_fg)
                .hover(|s| s.bg(gpui::rgba(0xffffff22)).text_color(theme.fg))
                .child("×")
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    cx.stop_propagation();
                    optimistic_close(&close_entity, history_id, cx);
                    dispatch_tab_op(&close_entity, &close_services, close_intent(history_id), cx);
                })
                .into_any_element(),
        )
        .with_displayed_text("×");

        let switch_services = services.clone();
        let switch_entity = state_entity.clone();
        let mut row = div()
            .id(SharedString::from(format!("tab-list-row-el-{history_id}")))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .child(div().flex_1().child(caption.clone()));
        if is_active {
            row = row.bg(theme.selected_bg).text_color(theme.selected_fg);
        } else {
            row = row
                .cursor_pointer()
                .hover(|s| s.bg(gpui::rgba(0xffffff14)))
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    optimistic_activate(&switch_entity, history_id, cx);
                    dispatch_tab_op(
                        &switch_entity,
                        &switch_services,
                        activate_intent(history_id),
                        cx,
                    );
                    set_list_open(&switch_entity, false, cx);
                });
        }
        row = row.child(close_btn);

        panel = panel.child(
            crate::geometry::TransparentTracker::new(
                tab_list_row_id(history_id),
                if is_active {
                    "tab_list_row_active"
                } else {
                    "tab_list_row"
                },
                bounds.clone(),
                row.into_any_element(),
            )
            .with_displayed_text(caption),
        );
    }

    let new_services = services.clone();
    let new_entity = state_entity.clone();
    panel = panel.child(
        crate::geometry::TransparentTracker::new(
            TAB_LIST_NEW_ID.to_string(),
            "tab_list_new",
            bounds.clone(),
            div()
                .id("tab-list-new-el")
                .mt(px(4.0))
                .pt(px(6.0))
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .border_t_1()
                .border_color(theme.border)
                .cursor_pointer()
                .text_color(theme.muted_fg)
                .hover(|s| s.bg(gpui::rgba(0xffffff14)).text_color(theme.fg))
                .child("+ New tab")
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    // A new tab's row id is minted by the engine, so there is
                    // nothing to show optimistically — the count follows when
                    // the write reports back.
                    dispatch_tab_op(&new_entity, &new_services, new_tab_intent(), cx);
                    set_list_open(&new_entity, false, cx);
                })
                .into_any_element(),
        )
        .with_displayed_text("+ New tab"),
    );

    Some(list_backdrop(state_entity).child(panel).into_any_element())
}

/// The list's floating panel: same box whether it holds tabs or the reason
/// there are none to show.
fn tab_list_panel(theme: SearchTheme, anchor_top: f32) -> gpui::Stateful<gpui::Div> {
    div()
        .id("tab-list-panel")
        .absolute()
        .top(px(anchor_top))
        .right(px(12.0))
        .w(px(280.0))
        .max_h(px(420.0))
        .overflow_y_scroll()
        .bg(theme.bg)
        .rounded(px(10.0))
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .p(px(6.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .text_size(px(13.0))
        .text_color(theme.fg)
}

/// Full-window catcher behind the panel: a click anywhere outside closes the
/// list.
fn list_backdrop(state_entity: Entity<TabStripState>) -> gpui::Stateful<gpui::Div> {
    div()
        .id("tab-list-overlay")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
            set_list_open(&state_entity, false, cx);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(ids: &[i64]) -> Vec<TabEntry> {
        ids.iter()
            .map(|&history_id| TabEntry {
                history_id,
                block_id: Some(EntityUri::block(&format!("b{history_id}"))),
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

    #[test]
    fn neighbor_after_close_prefers_left() {
        let t = tabs(&[10, 20, 30]);
        // Closing the middle tab follows LEFT to its predecessor.
        assert_eq!(neighbor_after_close(&t, 20), Some(10));
        // Closing the last tab follows LEFT to its predecessor.
        assert_eq!(neighbor_after_close(&t, 30), Some(20));
    }

    #[test]
    fn neighbor_after_close_falls_back_right_then_none() {
        let t = tabs(&[10, 20, 30]);
        // Closing the leftmost tab has no left neighbor -> falls back RIGHT.
        assert_eq!(neighbor_after_close(&t, 10), Some(20));
        // The only tab -> nothing to follow to.
        assert_eq!(neighbor_after_close(&tabs(&[10]), 10), None);
        // Unknown id -> None.
        assert_eq!(neighbor_after_close(&t, 999), None);
    }
}
