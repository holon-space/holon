//! Page-ancestor breadcrumb for the focused page (dogfood #5: no path back
//! after navigating into a nested page).
//!
//! Resolves the focused block's `Page`-ancestor trail through the query
//! capability ([`holon_api::QueryEngine::breadcrumb_trail`], which reuses the
//! `block_with_path` matview — no separate tree walk) and renders it as
//! clickable segments. A segment click navigates through the SAME chokepoint
//! quick-open and the sidebar use ([`crate::search_ui::navigate_to`]).
//!
//! Async resolution mirrors `search_ui::run_search`: `HolonApp::render` detects
//! a focus change and calls [`resolve_breadcrumb`], which runs the query on
//! tokio and pumps the trail back into this entity, emitting
//! [`NotifyBreadcrumb`] to re-render.
//!
//! Fail-loud: a trail that can't be resolved lands in `error` and is rendered
//! as a visible message, never a silently-empty bar.

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
use holon_frontend::FrontendSession;
use holon_frontend::reactive::BuilderServices;

use crate::search_ui::Hit;
use crate::search_ui::SearchTheme;
use crate::search_ui::navigate_to;

/// Focused-page breadcrumb state. Its own `Entity` so async trail resolution
/// can update it and trigger a re-render (same pattern as `SearchUiState`).
pub struct BreadcrumbState {
    /// The block whose trail is currently shown/being resolved.
    pub block_id: Option<EntityUri>,
    /// Page ancestors, root → current.
    pub segments: Vec<Hit>,
    pub error: Option<String>,
    /// Drops stale async responses when focus changes rapidly.
    pub generation: u64,
}

impl Default for BreadcrumbState {
    fn default() -> Self {
        Self {
            block_id: None,
            segments: Vec::new(),
            error: None,
            generation: 0,
        }
    }
}

pub struct NotifyBreadcrumb;
impl EventEmitter<NotifyBreadcrumb> for BreadcrumbState {}

/// Resolve the breadcrumb trail for `block_id` and pump it back into `state`.
pub fn resolve_breadcrumb(
    block_id: EntityUri,
    generation: u64,
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    state: Entity<BreadcrumbState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) {
    let (tx, rx) =
        futures::channel::oneshot::channel::<Result<Vec<holon_api::LinkCandidate>, String>>();
    let query_block = block_id.clone();
    rt_handle.spawn(async move {
        let outcome = match session.query_engine() {
            Some(qe) => qe
                .breadcrumb_trail(&query_block)
                .await
                .map_err(|e| format!("{e:#}")),
            None => Err("breadcrumb needs the Turso query backend".to_string()),
        };
        let _ = tx.send(outcome);
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                state.update(cx, |s, cx| {
                    if s.generation == generation {
                        match outcome {
                            Ok(Ok(trail)) => {
                                s.segments = trail
                                    .into_iter()
                                    .map(|c| Hit {
                                        id: c.id,
                                        label: c.label,
                                    })
                                    .collect();
                                s.error = None;
                            }
                            Ok(Err(e)) => {
                                s.segments.clear();
                                s.error = Some(e);
                            }
                            Err(_cancelled) => {
                                s.error = Some("breadcrumb task dropped".to_string());
                            }
                        }
                        cx.emit(NotifyBreadcrumb);
                        cx.notify();
                    }
                });
            });
        })
        .detach();
}

fn seg_title(label: &str) -> String {
    let first = label.lines().next().unwrap_or("").trim();
    const MAX: usize = 32;
    if first.chars().count() > MAX {
        let cut: String = first.chars().take(MAX).collect();
        format!("{cut}…")
    } else {
        first.to_string()
    }
}

/// Which segments to show. Long trails collapse to `root … parent current`
/// (first + ellipsis + last two) so the bar never wraps; short trails show in
/// full. Returns display items where `None` marks the non-clickable ellipsis.
fn displayed_segments(segments: &[Hit]) -> Vec<Option<(usize, &Hit)>> {
    const KEEP_TAIL: usize = 2;
    if segments.len() <= KEEP_TAIL + 2 {
        return segments.iter().enumerate().map(Some).collect();
    }
    let mut out: Vec<Option<(usize, &Hit)>> = Vec::new();
    out.push(Some((0, &segments[0])));
    out.push(None); // ellipsis
    let start = segments.len() - KEEP_TAIL;
    for (i, seg) in segments.iter().enumerate().skip(start) {
        out.push(Some((i, seg)));
    }
    out
}

/// Render the breadcrumb bar for the currently-focused page. `None` when there
/// is nothing to show (no focus / trail not yet resolved and no error).
pub fn render_breadcrumb_bar(
    state_read: &BreadcrumbState,
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
                .child(format!("Breadcrumb unavailable: {err}"))
                .into_any_element(),
        );
    }
    if state_read.segments.is_empty() {
        return None;
    }

    let last_idx = state_read.segments.len() - 1;
    let mut bar = div()
        .id("breadcrumb-bar")
        .flex()
        .flex_row()
        .items_center()
        .flex_wrap()
        .h(px(28.0))
        .pl(px(left_pad))
        .pr(px(16.0))
        .gap(px(4.0))
        .border_b_1()
        .border_color(theme.border)
        .text_size(px(12.0))
        .text_color(theme.muted_fg);

    let mut first = true;
    for item in displayed_segments(&state_read.segments) {
        if !first {
            bar = bar.child(div().text_color(theme.muted_fg).child("›"));
        }
        first = false;
        match item {
            None => {
                bar = bar.child(div().child("…"));
            }
            Some((idx, seg)) => {
                let is_current = idx == last_idx;
                let services = services.clone();
                let target = seg.id.clone();
                let seg_fg = if is_current { theme.fg } else { theme.muted_fg };
                let mut chip = div()
                    .id(SharedString::from(format!("breadcrumb-seg-{idx}")))
                    .px(px(4.0))
                    .rounded(px(4.0))
                    .text_color(seg_fg)
                    .child(seg_title(&seg.label));
                if !is_current {
                    chip = chip
                        .cursor_pointer()
                        .hover(|s| s.bg(gpui::rgba(0xffffff14)))
                        .on_mouse_down(MouseButton::Left, move |_, _window, _cx| {
                            navigate_to(&services, &target);
                        });
                }
                bar = bar.child(chip);
            }
        }
    }

    Some(bar.into_any_element())
}

#[cfg(test)]
mod tests {
    use holon_api::EntityUri;

    use super::*;

    fn hits(n: usize) -> Vec<Hit> {
        (0..n)
            .map(|i| Hit {
                id: EntityUri::parse(&format!("block:seg{i}")).unwrap(),
                label: format!("Page {i}"),
            })
            .collect()
    }

    #[test]
    fn short_trail_shows_all_segments() {
        let segs = hits(3);
        let shown = displayed_segments(&segs);
        assert_eq!(shown.len(), 3);
        assert!(shown.iter().all(|s| s.is_some()));
    }

    #[test]
    fn long_trail_collapses_to_root_ellipsis_tail() {
        let segs = hits(6);
        let shown = displayed_segments(&segs);
        // root + ellipsis + last two
        assert_eq!(shown.len(), 4);
        assert_eq!(shown[0].map(|(i, _)| i), Some(0));
        assert!(shown[1].is_none()); // ellipsis
        assert_eq!(shown[2].map(|(i, _)| i), Some(4));
        assert_eq!(shown[3].map(|(i, _)| i), Some(5));
    }

    #[test]
    fn seg_title_first_line_and_caps() {
        assert_eq!(seg_title("Root\ndetails"), "Root");
        let long = "y".repeat(50);
        let out = seg_title(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 33); // 32 + ellipsis
    }
}
