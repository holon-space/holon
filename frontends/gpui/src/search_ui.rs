//! GPUI user-facing search modal: quick-open (jump to page/block) + full-text
//! content search, in one `cmd-K` overlay.
//!
//! Two result sections are rendered: **Pages** first (jump-to-page targets) and
//! **In content** second (full-text block matches). The split is computed
//! behind the query capability ([`holon_api::QueryEngine::quick_open_search`]),
//! so the frontend only routes typed hits, never SQL.
//!
//! Async bridge (mirrors `share_ui`): a keystroke fires
//! [`InputEvent::Change`], the subscription reads the query and calls
//! [`run_search`], which `rt_handle.spawn`s the query on tokio and pumps the
//! result back through `async_cx.spawn` → `cx.update_window` → the
//! `SearchUiState` entity, emitting [`NotifySearchUi`] to re-render `HolonApp`.
//! A `generation` counter drops out-of-order (stale) responses.
//!
//! Navigation reuses the ONE navigation chokepoint the sidebar and wiki-links
//! use: `dispatch_intent(navigation.focus{region:"main", block_id})`.

use std::sync::Arc;

use gpui::AnyWindowHandle;
use gpui::AsyncApp;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::Hsla;
use gpui::MouseButton;
use gpui::SharedString;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui_component::input::Input;
use gpui_component::input::InputState;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;

/// One search hit — a typed target the modal can navigate to.
#[derive(Debug, Clone)]
pub struct Hit {
    pub id: EntityUri,
    pub label: String,
}

/// Theme colors for the search overlay (passed from `HolonApp::render`).
#[derive(Clone, Copy)]
pub struct SearchTheme {
    pub bg: Hsla,
    pub border: Hsla,
    pub fg: Hsla,
    pub muted_fg: Hsla,
    pub selected_bg: Hsla,
    pub selected_fg: Hsla,
    /// Secondary text on the selection fill — `muted_fg` is mixed for the page
    /// background and reads at ~1:1 on the accent. See
    /// [`holon_frontend::theme::muted_on_selection`].
    pub selected_muted_fg: Hsla,
}

/// The colours one hit row paints.
pub struct RowColors {
    pub bg: Hsla,
    pub fg: Hsla,
    pub subtitle_fg: Hsla,
}

impl SearchTheme {
    /// A row carrying the selection fill is ONE state, whichever authority put
    /// it there — the keyboard selection or the pointer. Deriving all three
    /// colours from that single bool is what keeps the subtitle from staying at
    /// the page's `muted_fg` (a ~1:1 ghost) on an accent-filled row.
    pub fn row_colors(&self, filled: bool) -> RowColors {
        if filled {
            RowColors {
                bg: self.selected_bg,
                fg: self.selected_fg,
                subtitle_fg: self.selected_muted_fg,
            }
        } else {
            RowColors {
                bg: self.bg,
                fg: self.fg,
                subtitle_fg: self.muted_fg,
            }
        }
    }
}

/// Per-window search-modal state. Lives in its own `Entity` so async query
/// results can update it (and trigger a re-render) without going through the
/// reactive engine — same pattern as `ShareUiState`.
pub struct SearchUiState {
    pub open: bool,
    pub input: Entity<InputState>,
    pub pages: Vec<Hit>,
    pub content: Vec<Hit>,
    pub error: Option<String>,
    /// Index into the flattened `pages ++ content` list.
    pub selected: usize,
    /// Flattened index the pointer is over, if any. Hover carries the same
    /// selection fill as the keyboard selection, so it must be real state the
    /// row colours derive from — a `hover` style closure could only repaint the
    /// row's own background and would leave its subtitle at the page's
    /// `muted_fg` on the accent.
    pub hovered: Option<usize>,
    pub query: String,
    /// Bumped on every keystroke; async responses carrying an older value are
    /// dropped so a slow query can't overwrite a newer one.
    pub generation: u64,
    /// Soft-keyboard focus generation this modal claimed on its last open (0 if
    /// never opened). The search box is a `gpui_component` text input just like
    /// an editor block, so on mobile it must join the SAME keyboard-generation
    /// protocol (`crate::soft_keyboard::editor_focus_gained`/
    /// `editor_focus_lost`):
    ///   * On open it claims a generation and raises the keyboard — without
    ///     this the search box shows a caret but no keyboard.
    ///   * Claiming a generation also CANCELS the deferred-hide the
    ///     just-blurred editor scheduled (its `my_generation` is now stale), so
    ///     opening search over a focused block keeps the keyboard up instead of
    ///     letting it drop ~150ms later.
    focus_gen: u64,
    /// Window focus at the moment `open` stole it, handed back by `close`.
    ///
    /// The editor→window focus bridge (`editor_view::spawn_focus_binding`) is
    /// deduped on the `focused_block` signal, and closing the overlay does not
    /// move that signal — so nothing re-grabs focus on its own and the window
    /// is left focusing a widget that is no longer rendered.
    restore_focus: Option<gpui::FocusHandle>,
}

/// Emitted whenever the search state changes so `HolonApp` re-renders.
pub struct NotifySearchUi;
impl EventEmitter<NotifySearchUi> for SearchUiState {}

impl SearchUiState {
    pub fn new(window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search pages and content…"));
        Self {
            open: false,
            input,
            pages: Vec::new(),
            content: Vec::new(),
            error: None,
            selected: 0,
            hovered: None,
            query: String::new(),
            generation: 0,
            focus_gen: 0,
            restore_focus: None,
        }
    }

    /// Total number of navigable hits across both sections.
    pub fn total_hits(&self) -> usize {
        self.pages.len() + self.content.len()
    }

    /// Resolve the currently-selected hit (flattened pages-then-content).
    pub fn selected_hit(&self) -> Option<&Hit> {
        self.pages
            .iter()
            .chain(self.content.iter())
            .nth(self.selected)
    }

    /// Open the modal: reset state and focus the input.
    pub fn open(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.open = true;
        self.query.clear();
        self.pages.clear();
        self.content.clear();
        self.error = None;
        self.selected = 0;
        self.hovered = None;
        self.generation = self.generation.wrapping_add(1);
        self.restore_focus = window.focused(cx);
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        // Mobile: focusing the input renders a caret but does NOT raise the
        // platform keyboard on its own (the fork only shows it via
        // `show_keyboard`). Claim a keyboard generation and show it — this both
        // pops the keyboard for the search box and supersedes any deferred-hide
        // the editor we just blurred scheduled. See `focus_gen`.
        self.focus_gen = crate::soft_keyboard::editor_focus_gained();
    }

    /// Close the modal and hand window focus back to whoever held it on
    /// `open`.
    ///
    /// The hand-back is a no-op on the navigating paths (Enter / result
    /// click): the destination becomes the main region's navigation root and
    /// renders through the editor-less `page_title` variant, so the recorded
    /// handle points at a widget that just unmounted. The keyboard survives
    /// anyway because the navigation SEATS a caret inside the destination
    /// (D97.a, `ReactiveEngine::spawn_caret_seat`) and the editor mounted
    /// there grabs window focus. Bugfunnel
    /// `2026-09-08-quick-open-enter-navigation-leaves-no-editable-focus`.
    pub fn close(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.open = false;
        if let Some(handle) = self.restore_focus.take() {
            window.focus(&handle, cx);
        }
        // Dismiss the soft keyboard (generation-guarded, so it is a no-op if
        // focus has since moved to another text input). An explicit dismissal,
        // not a focus-out event — the modal is going away — so it goes
        // straight to `editor_focus_lost`.
        crate::soft_keyboard::editor_focus_lost(cx, self.focus_gen);
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.selected = clamp_selection(self.selected, delta, self.total_hits());
    }
}

/// Clamp a selection index after moving by `delta` within `total` items.
/// Empty lists pin to 0; movement never escapes `[0, total-1]`.
pub fn clamp_selection(current: usize, delta: isize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let moved = current as isize + delta;
    moved.clamp(0, total as isize - 1) as usize
}

/// Kick off an async quick-open search and pump results back into `state`.
///
/// Mirrors `share_ui::dispatch_share`: the query runs on tokio (`rt_handle`),
/// the result is routed back onto GPUI's executor (`async_cx`). Fails loud —
/// a missing query backend or a query error lands in `state.error`, which the
/// overlay renders instead of silently showing no results.
pub fn run_search(
    query: String,
    generation: u64,
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    state: Entity<SearchUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) {
    let (tx, rx) =
        futures::channel::oneshot::channel::<Result<holon_api::QuickOpenResults, String>>();
    let query_for_log = query.clone();
    rt_handle.spawn(async move {
        let outcome = match session.query_engine() {
            Some(qe) => qe
                .quick_open_search(&query)
                .await
                .map_err(|e| format!("{e:#}")),
            None => Err(
                "Search needs the Turso query backend, which this session doesn't have".to_string(),
            ),
        };
        if let Err(unsent) = tx.send(outcome) {
            tracing::error!(
                query = %query,
                outcome = ?unsent,
                "quick_open_search: the overlay dropped its receiver before the result arrived"
            );
        }
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let delivered = cx.update_window(window_handle, |_, _window, cx| {
                state.update(cx, |s, cx| {
                    // Drop stale responses (a newer keystroke already fired).
                    if s.generation == generation {
                        match outcome {
                            Ok(Ok(results)) => {
                                s.pages = results
                                    .pages
                                    .into_iter()
                                    .map(|c| Hit {
                                        id: c.id,
                                        label: c.label,
                                    })
                                    .collect();
                                s.content = results
                                    .content
                                    .into_iter()
                                    .map(|c| Hit {
                                        id: c.id,
                                        label: c.label,
                                    })
                                    .collect();
                                s.error = None;
                                s.selected = 0;
                                // A new result list re-numbers the rows, so the
                                // index the pointer was over names a different
                                // hit now.
                                s.hovered = None;
                            }
                            Ok(Err(e)) => {
                                s.pages.clear();
                                s.content.clear();
                                s.error = Some(e);
                            }
                            Err(_cancelled) => {
                                s.error = Some("search task dropped before responding".to_string());
                            }
                        }
                        cx.emit(NotifySearchUi);
                        cx.notify();
                    }
                });
            });
            if let Err(e) = delivered {
                tracing::error!(
                    query = %query_for_log,
                    error = %e,
                    "quick_open_search: results could not reach the overlay — the window is gone"
                );
            }
        })
        .detach();
}

/// Navigate the main region to `target` through the shared navigation
/// chokepoint (same intent the sidebar and wiki-links dispatch).
pub fn navigate_to(services: &Arc<dyn BuilderServices>, target: &EntityUri) {
    services.dispatch_intent(OperationIntent::new(
        "navigation".into(),
        "focus".into(),
        [
            ("region".to_string(), Value::String("main".to_string())),
            ("block_id".to_string(), Value::String(target.to_string())),
        ]
        .into_iter()
        .collect(),
    ));
}

/// Element ids the layout record keys a hit row's parts under. `idx` is the
/// flattened pages-then-content position, the same index the keyboard
/// selection uses.
pub fn hit_row_id(idx: usize) -> String {
    format!("search-hit-{idx}")
}

pub fn hit_title_id(idx: usize) -> String {
    format!("search-hit-{idx}-title")
}

pub fn hit_subtitle_id(idx: usize) -> String {
    format!("search-hit-{idx}-subtitle")
}

fn truncate_label(label: &str) -> String {
    let first_line = label.lines().next().unwrap_or("").trim();
    const MAX: usize = 90;
    if first_line.chars().count() > MAX {
        let cut: String = first_line.chars().take(MAX).collect();
        format!("{cut}…")
    } else {
        first_line.to_string()
    }
}

/// Build the search overlay when `open`. Returns `None` when closed.
///
/// Keyboard: `Escape` closes, `Enter` navigates to the selected hit, `Up`/
/// `Down` move the selection. Handled on the overlay (bubble phase) so normal
/// typing still reaches the focused input.
#[allow(clippy::too_many_arguments)]
pub fn render_search_overlay(
    state_read: &SearchUiState,
    state_entity: Entity<SearchUiState>,
    services: Arc<dyn BuilderServices>,
    bounds: crate::geometry::BoundsRegistry,
    theme: SearchTheme,
) -> Option<gpui::Stateful<gpui::Div>> {
    if !state_read.open {
        return None;
    }

    let overlay_bg = gpui::rgba(0x00000088);
    let input = state_read.input.clone();
    let selected = state_read.selected;
    let hovered = state_read.hovered;

    // Rows, flattened pages-then-content, so a click maps to the same index the
    // keyboard selection uses.
    let mut rows: Vec<gpui::AnyElement> = Vec::new();
    let mut flat_idx = 0usize;

    let push_section =
        |rows: &mut Vec<gpui::AnyElement>, flat_idx: &mut usize, title: &str, hits: &[Hit]| {
            if hits.is_empty() {
                return;
            }
            rows.push(
                div()
                    .px(px(8.0))
                    .pt(px(8.0))
                    .pb(px(4.0))
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.muted_fg)
                    .child(title.to_string())
                    .into_any_element(),
            );
            for hit in hits {
                let idx = *flat_idx;
                *flat_idx += 1;
                let is_selected = idx == selected;
                let services = services.clone();
                let state_entity = state_entity.clone();
                let target = hit.id.clone();
                let RowColors {
                    bg: row_bg,
                    fg: row_fg,
                    subtitle_fg,
                } = theme.row_colors(is_selected || hovered == Some(idx));
                let subtitle = target.to_string();
                rows.push(
                    crate::geometry::TransparentTracker::new(
                        hit_row_id(idx),
                        "search_hit_row",
                        bounds.clone(),
                        div()
                            .id(SharedString::from(format!("search-hit-{idx}")))
                            .flex()
                            .flex_col()
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(row_bg)
                            .text_color(row_fg)
                            .cursor_pointer()
                            .on_hover({
                                let state_entity = state_entity.clone();
                                move |hovering, _window, cx| {
                                    state_entity.update(cx, |s, cx| {
                                        let next = if *hovering { Some(idx) } else { None };
                                        if !*hovering && s.hovered != Some(idx) {
                                            return;
                                        }
                                        if s.hovered != next {
                                            s.hovered = next;
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                            .child(
                                crate::geometry::TransparentTracker::new(
                                    hit_title_id(idx),
                                    "search_hit_title",
                                    bounds.clone(),
                                    div()
                                        .text_size(px(14.0))
                                        .child(truncate_label(&hit.label))
                                        .into_any_element(),
                                )
                                .with_displayed_text(truncate_label(&hit.label)),
                            )
                            .child(
                                crate::geometry::TransparentTracker::new(
                                    hit_subtitle_id(idx),
                                    "search_hit_subtitle",
                                    bounds.clone(),
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(subtitle_fg)
                                        .child(subtitle.clone())
                                        .into_any_element(),
                                )
                                .with_painted_colors(Some(subtitle_fg), None)
                                .with_displayed_text(subtitle),
                            )
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                navigate_to(&services, &target);
                                state_entity.update(cx, |s, cx| {
                                    s.close(window, cx);
                                    cx.emit(NotifySearchUi);
                                    cx.notify();
                                });
                            })
                            .into_any_element(),
                    )
                    .with_painted_colors(Some(row_fg), Some(row_bg))
                    .into_any_element(),
                );
            }
        };

    push_section(&mut rows, &mut flat_idx, "Pages", &state_read.pages);
    push_section(&mut rows, &mut flat_idx, "In content", &state_read.content);

    let body: gpui::AnyElement = if let Some(err) = &state_read.error {
        div()
            .p(px(12.0))
            .text_size(px(13.0))
            .text_color(gpui::rgb(0xd9534f))
            .child(format!("Search failed: {err}"))
            .into_any_element()
    } else if rows.is_empty() {
        let msg = if state_read.query.trim().is_empty() {
            "Type to search pages and content".to_string()
        } else {
            format!("No matches for \"{}\"", state_read.query.trim())
        };
        div()
            .p(px(12.0))
            .text_size(px(13.0))
            .text_color(theme.muted_fg)
            .child(msg)
            .into_any_element()
    } else {
        let mut list = div().flex().flex_col().gap(px(2.0));
        for r in rows {
            list = list.child(r);
        }
        list.into_any_element()
    };

    // Keyboard handler on the panel (bubble phase — after the input).
    let key_state = state_entity.clone();
    let key_services = services.clone();

    let overlay = div()
        .id("search-overlay")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(overlay_bg)
        .flex()
        .flex_col()
        .items_center()
        .pt(px(80.0))
        .px(px(16.0))
        .on_key_down(move |ev, window, cx| {
            let key = ev.keystroke.key.as_str();
            match key {
                "escape" => {
                    key_state.update(cx, |s, cx| {
                        s.close(window, cx);
                        cx.emit(NotifySearchUi);
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
                "enter" => {
                    let target = key_state.read(cx).selected_hit().map(|h| h.id.clone());
                    if let Some(target) = target {
                        navigate_to(&key_services, &target);
                        key_state.update(cx, |s, cx| {
                            s.close(window, cx);
                            cx.emit(NotifySearchUi);
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }
                }
                "down" => {
                    key_state.update(cx, |s, cx| {
                        s.move_selection(1);
                        cx.emit(NotifySearchUi);
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
                "up" => {
                    key_state.update(cx, |s, cx| {
                        s.move_selection(-1);
                        cx.emit(NotifySearchUi);
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
                _ => {}
            }
        })
        .child(
            div()
                .id("search-panel")
                // Click-away dismiss: fires only for a mouse-down OUTSIDE this
                // panel (same idiom as `lib.rs::modal_overlay`), so clicks on
                // the input / results never close the modal.
                .on_mouse_down_out({
                    let state_entity = state_entity.clone();
                    move |_, window, cx| {
                        state_entity.update(cx, |s, cx| {
                            s.close(window, cx);
                            cx.emit(NotifySearchUi);
                            cx.notify();
                        });
                    }
                })
                .w_full()
                .max_w(px(640.0))
                .max_h(px(560.0))
                .bg(theme.bg)
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .p(px(12.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .child(Input::new(&input)),
                )
                .child(
                    div()
                        .id("search-results")
                        .flex_1()
                        .overflow_y_scroll()
                        .p(px(6.0))
                        .child(body),
                ),
        );

    Some(overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_selection_empty_pins_to_zero() {
        assert_eq!(clamp_selection(0, 1, 0), 0);
        assert_eq!(clamp_selection(5, -1, 0), 0);
    }

    #[test]
    fn clamp_selection_stays_in_bounds() {
        assert_eq!(clamp_selection(0, -1, 3), 0); // no wrap past top
        assert_eq!(clamp_selection(0, 1, 3), 1);
        assert_eq!(clamp_selection(2, 1, 3), 2); // no wrap past bottom
        assert_eq!(clamp_selection(2, -1, 3), 1);
    }

    #[test]
    fn truncate_label_takes_first_line_and_caps_length() {
        assert_eq!(truncate_label("Hello\nworld"), "Hello");
        assert_eq!(truncate_label("  spaced  "), "spaced");
        let long = "x".repeat(200);
        let out = truncate_label(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 91); // 90 + ellipsis
    }
}
