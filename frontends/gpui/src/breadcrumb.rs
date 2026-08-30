//! Page-ancestor breadcrumb for the current view (dogfood #5: no path back
//! after navigating into a nested page).
//!
//! The bar shows the path of what the user is looking at: the focused block
//! when there is one, otherwise the Main region's view root. It therefore moves
//! on navigation as well as on focus, and a cold boot onto an open page draws
//! the trail before the user touches anything.
//!
//! Resolves the source block's `Page`-ancestor trail through the query
//! capability ([`holon_api::QueryEngine::breadcrumb_trail`], which reuses the
//! `block_with_path` matview — no separate tree walk) and renders it as
//! clickable segments. A segment click navigates through the SAME chokepoint
//! quick-open and the sidebar use ([`crate::search_ui::navigate_to`]).
//!
//! Async resolution mirrors `search_ui::run_search`: `HolonApp::render` detects
//! a focus or navigation change and calls [`resolve_breadcrumb`], which runs
//! the queries on tokio and pumps the trail back into this entity, emitting
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

/// Current-view breadcrumb state. Its own `Entity` so async trail resolution
/// can update it and trigger a re-render (same pattern as `SearchUiState`).
pub struct BreadcrumbState {
    /// The block whose trail is currently shown/being resolved.
    pub block_id: Option<EntityUri>,
    /// Page ancestors, root → current.
    pub segments: Vec<Hit>,
    pub error: Option<String>,
    /// Drops stale async responses when the bar is re-resolved rapidly.
    pub generation: u64,
    /// Main view root as of the last resolution that read one. A steal from a
    /// live caret requires this to have CHANGED — a bumped view generation on
    /// its own does not, since ops that cannot move the cursor still bump it.
    pub view_root: Option<EntityUri>,
}

impl Default for BreadcrumbState {
    fn default() -> Self {
        Self {
            block_id: None,
            segments: Vec::new(),
            error: None,
            generation: 0,
            view_root: None,
        }
    }
}

pub struct NotifyBreadcrumb;
impl EventEmitter<NotifyBreadcrumb> for BreadcrumbState {}

/// What one resolution decided: the block whose trail to draw, its page
/// ancestors, and the view root observed while deciding (`None` when the
/// resolution did not need to read one). The whole bar is `None` when there is
/// nothing to show at all — no focus and no open view.
struct Resolved {
    source: EntityUri,
    trail: Vec<holon_api::LinkCandidate>,
    view_root: Option<EntityUri>,
}

/// Which block the bar draws, given what moved since the last resolution.
///
/// A caret that moved wins. Otherwise the caret is presumed stale ONLY if the
/// view root really CHANGED — ops that cannot move the cursor (closing a
/// background tab) bump the generation too, and those must leave a live caret
/// alone.
async fn resolve_trail(
    focused: Option<EntityUri>,
    caret_moved: bool,
    view_moved: bool,
    last_view_root: Option<EntityUri>,
    session: &FrontendSession,
) -> Result<Option<Resolved>, String> {
    let qe = session
        .query_engine()
        .ok_or_else(|| "breadcrumb needs the Turso query backend".to_string())?;

    // The view generation did not move, so the root cannot have either: skip
    // reading it and carry the last one forward.
    if let (false, Some(block)) = (view_moved, focused.clone()) {
        let trail = qe
            .breadcrumb_trail(&block)
            .await
            .map_err(|e| format!("{e:#}"))?;
        return Ok(Some(Resolved {
            source: block,
            trail,
            view_root: last_view_root,
        }));
    }

    let root = qe
        .region_view_root(holon_api::Region::Main)
        .await
        .map_err(|e| format!("breadcrumb view root: {e:#}"))?;
    let source = match (&focused, &root) {
        (Some(caret), _) if caret_moved => caret.clone(),
        (Some(caret), Some(root)) if Some(root) == last_view_root.as_ref() => caret.clone(),
        (_, Some(root)) => root.clone(),
        (_, None) => return Ok(None),
    };
    let trail = qe
        .breadcrumb_trail(&source)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(Some(Resolved {
        source,
        trail,
        view_root: root,
    }))
}

/// Resolve the bar and pump it back into `state`. `caret_moved` says whether
/// the focus changed since the last resolution; see [`resolve_trail`].
pub fn resolve_breadcrumb(
    focused: Option<EntityUri>,
    caret_moved: bool,
    view_moved: bool,
    last_view_root: Option<EntityUri>,
    generation: u64,
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    state: Entity<BreadcrumbState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) {
    let (tx, rx) = futures::channel::oneshot::channel::<Result<Option<Resolved>, String>>();
    rt_handle.spawn(async move {
        let _ = tx
            .send(resolve_trail(focused, caret_moved, view_moved, last_view_root, &session).await);
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                state.update(cx, |s, cx| {
                    if s.generation == generation {
                        match outcome {
                            Ok(Ok(Some(resolved))) => {
                                s.block_id = Some(resolved.source);
                                s.view_root = resolved.view_root;
                                s.segments = resolved
                                    .trail
                                    .into_iter()
                                    .map(|c| Hit {
                                        id: c.id,
                                        label: c.label,
                                    })
                                    .collect();
                                s.error = None;
                            }
                            Ok(Ok(None)) => {
                                s.block_id = None;
                                s.view_root = None;
                                s.segments.clear();
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

/// The trail as the title row's own content: the same segments
/// the bar form had, without a height, padding or border of its own, and
/// laid out to give way — it takes the row's leftover width (`flex_1`) and
/// clips (`min_w_0` + `overflow_hidden`) so the toolbar to its right is never
/// pushed off a phone screen. `None` when there is nothing to show.
///
/// Long trails already collapse to `root … parent current` in
/// [`displayed_segments`]; on a narrow row the clip finishes the job.
pub fn render_breadcrumb_inline(
    state_read: &BreadcrumbState,
    services: Arc<dyn BuilderServices>,
    theme: SearchTheme,
) -> Option<gpui::AnyElement> {
    if let Some(err) = &state_read.error {
        return Some(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .overflow_hidden()
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
    let mut row = div()
        .id("breadcrumb-inline")
        .flex()
        .flex_row()
        .flex_1()
        .min_w_0()
        .items_center()
        .overflow_hidden()
        .gap(px(4.0))
        .text_size(px(12.0))
        .text_color(theme.muted_fg);

    let mut first = true;
    for item in displayed_segments(&state_read.segments) {
        if !first {
            row = row.child(div().text_color(theme.muted_fg).child("›"));
        }
        first = false;
        match item {
            None => {
                row = row.child(div().child("…"));
            }
            Some((idx, seg)) => {
                let is_current = idx == last_idx;
                let services = services.clone();
                let target = seg.id.clone();
                let seg_fg = if is_current { theme.fg } else { theme.muted_fg };
                let mut chip = div()
                    .id(SharedString::from(format!("breadcrumb-seg-{idx}")))
                    .flex_none()
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
                row = row.child(chip);
            }
        }
    }

    Some(row.into_any_element())
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
