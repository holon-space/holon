//! GPUI UI for subtree sharing: share/accept modals + degraded-event surfacing.
//!
//! Three user-visible entry points:
//! - **Share:** context menu on a block (wired in `lib.rs`) → "Share subtree"
//!   calls `execute_operation("tree", "share_subtree", ...)` and opens
//!   a modal with the returned ticket + a bearer-capability warning quoted
//!   from `docs/SUBTREE_SHARING.md` + a reserved area for degraded events.
//! - **Accept:** title-bar button "🔗" opens a modal; the current flow uses
//!   "Paste from clipboard + use focused block as parent" because wiring a
//!   full in-modal text-editing form requires gpui_component::input focus
//!   plumbing that's orthogonal to this pass.
//! - **Degraded signals:** a background task drains
//!   `LoroShareBackend::degraded_bus()` and renders toasts / a red modal
//!   for `SnapshotSaveFailed`, `SnapshotLoadFailed`, `RehydrationFailed`.
//!
//! Bridge from the tokio broadcast into GPUI's reactive model:
//! `rt_handle.spawn` runs the `recv().await` loop inside the tokio runtime
//! and forwards events through a `futures::channel::mpsc::unbounded` channel
//! to a pump running on GPUI's executor (`cx.spawn`). The pump calls
//! `cx.update_window` to mutate the `ShareUiState` entity, which emits a
//! `NotifyShareUi` event that triggers the main `HolonApp`'s re-render.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, AnyWindowHandle, AsyncApp, ClipboardItem, Entity, EventEmitter, Hsla,
    IntoElement, MouseButton, SharedString, Stateful,
};
use holon::sync::{ShareDegraded, ShareDegradedReason};
use holon_api::{EntityName, Value};
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::FrontendSession;

/// Threat-model sentences from `docs/SUBTREE_SHARING.md` (lines 34–35).
/// Quoted verbatim — users of the share UI must see the exact wording so
/// there's no doubt this is a bearer capability.
pub const BEARER_CAPABILITY_WARNING: &str = "A ticket is a bearer capability. Anyone who obtains it can read and write the shared subtree until the share is dropped. There is no authn/authz layer inside iroh — peer identity is the only gate, and the initial handshake does not verify \"who you are\" beyond a cryptographic node id.";

/// Parsed response from `share_subtree` — the op returns a JSON string in
/// `OperationResult::response` with `ticket`, `shared_tree_id`, `mount_block_id`.
#[derive(Clone, Debug)]
pub struct ShareTicket {
    pub ticket: String,
    pub shared_tree_id: String,
    pub mount_block_id: String,
}

impl ShareTicket {
    pub fn from_value(v: &Value) -> anyhow::Result<Self> {
        let Value::String(s) = v else {
            anyhow::bail!("share_subtree response is not a String: {v:?}");
        };
        let parsed: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| anyhow::anyhow!("share_subtree response not valid JSON: {e}; raw={s}"))?;
        let ticket = parsed
            .get("ticket")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("share_subtree response missing `ticket`"))?
            .to_string();
        let shared_tree_id = parsed
            .get("shared_tree_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("share_subtree response missing `shared_tree_id`"))?
            .to_string();
        let mount_block_id = parsed
            .get("mount_block_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("share_subtree response missing `mount_block_id`"))?
            .to_string();
        Ok(ShareTicket {
            ticket,
            shared_tree_id,
            mount_block_id,
        })
    }
}

/// A degraded-mode notification to render as a yellow toast.
#[derive(Clone, Debug)]
pub struct DegradedToast {
    pub kind: DegradedKind,
    pub shared_tree_id: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradedKind {
    /// Yellow — save will retry on next commit.
    SnapshotSaveFailed,
    /// Yellow — rehydration hiccup at startup, share may lag.
    RehydrationFailed,
    /// A plain info-style toast (used for "ticket copied").
    Info,
}

/// A red-modal quarantine event. Separate from `DegradedToast` because it
/// needs a distinct, persistent, full-screen treatment.
#[derive(Clone, Debug)]
pub struct QuarantineEvent {
    pub shared_tree_id: String,
    pub quarantine_path: String,
}

/// Per-window share-UI state. Lives as a GPUI `Entity` on the main thread.
pub struct ShareUiState {
    pub share_modal: Option<ShareTicket>,
    pub show_accept_modal: bool,
    pub toasts: Vec<DegradedToast>,
    pub quarantines: Vec<QuarantineEvent>,
    pub share_error: Option<String>,
    pub accept_error: Option<String>,
}

impl ShareUiState {
    pub fn new() -> Self {
        Self {
            share_modal: None,
            show_accept_modal: false,
            toasts: Vec::new(),
            quarantines: Vec::new(),
            share_error: None,
            accept_error: None,
        }
    }

    pub fn open_share(&mut self, ticket: ShareTicket) {
        self.share_error = None;
        self.share_modal = Some(ticket);
    }

    pub fn open_accept(&mut self) {
        self.accept_error = None;
        self.show_accept_modal = true;
    }

    pub fn close_share(&mut self) {
        self.share_modal = None;
        self.share_error = None;
    }

    pub fn close_accept(&mut self) {
        self.show_accept_modal = false;
        self.accept_error = None;
    }

    pub fn dismiss_toast(&mut self, index: usize) {
        if index < self.toasts.len() {
            self.toasts.remove(index);
        }
    }

    pub fn dismiss_quarantine(&mut self, index: usize) {
        if index < self.quarantines.len() {
            self.quarantines.remove(index);
        }
    }

    /// Route a broadcast event from the degraded bus into the right field.
    pub fn apply_degraded(&mut self, event: ShareDegraded) {
        match event.reason {
            ShareDegradedReason::SnapshotSaveFailed(detail) => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::SnapshotSaveFailed,
                    shared_tree_id: event.shared_tree_id,
                    detail,
                });
            }
            ShareDegradedReason::RehydrationFailed(detail) => {
                self.push_toast(DegradedToast {
                    kind: DegradedKind::RehydrationFailed,
                    shared_tree_id: event.shared_tree_id,
                    detail,
                });
            }
            ShareDegradedReason::SnapshotLoadFailed(path) => {
                self.quarantines.push(QuarantineEvent {
                    shared_tree_id: event.shared_tree_id,
                    quarantine_path: path,
                });
            }
        }
    }

    pub fn push_toast(&mut self, toast: DegradedToast) {
        const MAX_TOASTS: usize = 5;
        if self.toasts.len() >= MAX_TOASTS {
            self.toasts.remove(0);
        }
        self.toasts.push(toast);
    }
}

impl Default for ShareUiState {
    fn default() -> Self {
        Self::new()
    }
}

/// Marker event — consumers call `cx.notify()` when they see it.
pub struct NotifyShareUi;
impl EventEmitter<NotifyShareUi> for ShareUiState {}

/// GPUI global that routes a right-click-share event from a block view back
/// into the window-level share-UI wiring. Any GPUI view that knows a row_id
/// and receives a right-click dispatches `ShareTrigger::trigger(block_id, cx)`.
///
/// Set in `launch_holon_window_impl` after the share_backend is wired.
#[derive(Clone)]
pub struct ShareTrigger(Arc<dyn Fn(String, &mut gpui::App) + Send + Sync>);

impl ShareTrigger {
    pub fn new(f: impl Fn(String, &mut gpui::App) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn trigger(block_id: String, cx: &mut gpui::App) {
        if let Some(t) = cx.try_global::<ShareTrigger>().cloned() {
            (t.0)(block_id, cx);
        } else {
            tracing::warn!(
                "[share-ui] ShareTrigger global missing; share context menu is inert \
                 (iroh-sync disabled?)"
            );
        }
    }
}

impl gpui::Global for ShareTrigger {}

// ─── Degraded bus bridge ────────────────────────────────────────────────────

/// Spawn the tokio-broadcast → GPUI-entity bridge.
///
/// The `recv()` loop runs inside the tokio runtime (`rt_handle.spawn`). Each
/// received `ShareDegraded` is forwarded through an unbounded `mpsc` channel
/// to a pump running on GPUI's executor, which calls `cx.update_window` to
/// mutate the `ShareUiState`.
pub fn spawn_degraded_bus_bridge(
    backend: Arc<holon::sync::loro_share_backend::LoroShareBackend>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<ShareDegraded>();

    // Tokio side: recv from broadcast, forward to mpsc.
    rt_handle.spawn(async move {
        let mut bus_rx = backend.degraded_bus().subscribe();
        loop {
            match bus_rx.recv().await {
                Ok(event) => {
                    if tx.unbounded_send(event).is_err() {
                        return; // pump gone, exit
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[share-ui] degraded bus lagged by {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::info!("[share-ui] degraded bus closed; bridge exiting");
                    return;
                }
            }
        }
    });

    // GPUI side: drain mpsc, mutate state entity.
    async_cx
        .spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(event) = rx.next().await {
                let _ = cx.update_window(window_handle, |_, _window, cx| {
                    share_state.update(cx, |s, cx| {
                        s.apply_degraded(event.clone());
                        cx.emit(NotifyShareUi);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
}

// ─── Op dispatchers (tokio-side + GPUI-side result routing) ─────────────────

pub fn dispatch_share(
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
    block_id: String,
) {
    let (tx, rx) = futures::channel::oneshot::channel::<Result<ShareTicket, String>>();
    rt_handle.spawn(async move {
        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), Value::String(block_id));
        params.insert("retention".to_string(), Value::String("full".to_string()));
        let result = session
            .execute_operation(&EntityName::new("tree"), "share_subtree", params)
            .await;
        let outcome = match result {
            Ok(Some(v)) => ShareTicket::from_value(&v).map_err(|e| format!("{e:#}")),
            Ok(None) => Err("share_subtree returned no response".to_string()),
            Err(e) => Err(format!("{e:#}")),
        };
        let _ = tx.send(outcome);
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                share_state.update(cx, |s, cx| {
                    match outcome {
                        Ok(Ok(ticket)) => s.open_share(ticket),
                        Ok(Err(e)) => {
                            s.share_modal = None;
                            s.share_error = Some(e);
                        }
                        Err(_cancelled) => {
                            s.share_error =
                                Some("share_subtree task dropped before responding".into());
                        }
                    }
                    cx.emit(NotifyShareUi);
                    cx.notify();
                });
            });
        })
        .detach();
}

pub fn dispatch_accept(
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    share_state: Entity<ShareUiState>,
    window_handle: AnyWindowHandle,
    async_cx: &AsyncApp,
    parent_id: String,
    ticket: String,
) {
    let (tx, rx) = futures::channel::oneshot::channel::<Result<(), String>>();
    rt_handle.spawn(async move {
        let mut params = std::collections::HashMap::new();
        params.insert("parent_id".to_string(), Value::String(parent_id));
        params.insert("ticket".to_string(), Value::String(ticket));
        let result = session
            .execute_operation(&EntityName::new("tree"), "accept_shared_subtree", params)
            .await;
        let outcome = match result {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("{e:#}")),
        };
        let _ = tx.send(outcome);
    });

    async_cx
        .spawn(async move |cx| {
            let outcome = rx.await;
            let _ = cx.update_window(window_handle, |_, _window, cx| {
                share_state.update(cx, |s, cx| {
                    match outcome {
                        Ok(Ok(())) => s.close_accept(),
                        Ok(Err(e)) => s.accept_error = Some(e),
                        Err(_) => {
                            s.accept_error = Some("accept_shared_subtree task dropped".into());
                        }
                    }
                    cx.emit(NotifyShareUi);
                    cx.notify();
                });
            });
        })
        .detach();
}

// ─── Rendering ──────────────────────────────────────────────────────────────

/// Theme values needed by the overlays.
#[derive(Clone, Copy)]
pub struct OverlayTheme {
    pub bg: Hsla,
    pub border: Hsla,
    pub fg: Hsla,
    pub muted_fg: Hsla,
}

/// Render every overlay (share/accept/quarantine modals + toast stack) for
/// the current state. Caller stacks these on top of the main content.
pub fn render_overlays(
    state: &ShareUiState,
    share_state: Entity<ShareUiState>,
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    window_handle: AnyWindowHandle,
    async_cx: AsyncApp,
    theme: OverlayTheme,
) -> Vec<AnyElement> {
    let mut overlays: Vec<AnyElement> = Vec::new();

    if let Some(ticket) = &state.share_modal {
        overlays.push(render_share_modal(ticket, share_state.clone(), theme));
    } else if let Some(e) = &state.share_error {
        overlays.push(render_error_modal(
            "Share failed",
            e,
            share_state.clone(),
            |s| s.share_error = None,
            theme,
        ));
    }

    if state.show_accept_modal {
        overlays.push(render_accept_modal(
            state.accept_error.as_deref(),
            share_state.clone(),
            session,
            engine,
            rt_handle,
            window_handle,
            async_cx,
            theme,
        ));
    }

    for (idx, q) in state.quarantines.iter().enumerate() {
        overlays.push(render_quarantine_modal(idx, q, share_state.clone(), theme));
    }

    if !state.toasts.is_empty() {
        overlays.push(render_toast_stack(&state.toasts, share_state, theme));
    }

    overlays
}

fn overlay_backdrop(id: &str) -> Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("{id}-backdrop")))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(gpui::rgba(0x00000088))
        .flex()
        .items_center()
        .justify_center()
}

fn modal_panel(id: &str, width: f32, theme: OverlayTheme) -> Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("{id}-panel")))
        .w(px(width))
        .max_h(px(720.0))
        .overflow_y_scroll()
        .bg(theme.bg)
        .rounded(px(12.0))
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .p(px(24.0))
        .flex()
        .flex_col()
        .gap_3()
}

fn render_share_modal(
    ticket: &ShareTicket,
    share_state: Entity<ShareUiState>,
    theme: OverlayTheme,
) -> AnyElement {
    let ticket_text = ticket.ticket.clone();
    let ticket_for_copy = ticket_text.clone();
    let shared_tree_id = ticket.shared_tree_id.clone();
    let mount_block_id = ticket.mount_block_id.clone();

    let close_a = share_state.clone();
    let close_b = share_state.clone();
    let copy_state = share_state.clone();

    overlay_backdrop("share-modal")
        .child(
            modal_panel("share-modal", 640.0, theme)
                .text_color(theme.fg)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .pb_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Subtree shared"),
                        )
                        .child(
                            div()
                                .id("share-modal-close")
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded(px(4.0))
                                .hover(|s| s.bg(gpui::rgba(0xffffff18)))
                                .child("✕")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_a.update(cx, |s, cx| {
                                        s.close_share();
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                .child(
                    div()
                        .p_3()
                        .rounded(px(6.0))
                        .bg(gpui::rgba(0x80000020))
                        .border_1()
                        .border_color(gpui::rgba(0xa02020ff))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .child(BEARER_CAPABILITY_WARNING.to_string()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_fg)
                        .child(format!(
                            "shared_tree_id: {shared_tree_id}   mount_block_id: {mount_block_id}"
                        )),
                )
                .child(
                    div()
                        .id("share-ticket-box")
                        .p_3()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(gpui::rgba(0x0000001a))
                        .text_size(px(11.0))
                        .text_color(theme.fg)
                        .child(ticket_text),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .id("share-copy-button")
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .bg(gpui::rgba(0x2563ebff))
                                .text_color(gpui::rgba(0xffffffff))
                                .text_size(px(13.0))
                                .hover(|s| s.bg(gpui::rgba(0x1d4ed8ff)))
                                .child("Copy ticket")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        ticket_for_copy.clone(),
                                    ));
                                    copy_state.update(cx, |s, cx| {
                                        s.push_toast(DegradedToast {
                                            kind: DegradedKind::Info,
                                            shared_tree_id: "ui".into(),
                                            detail: "Ticket copied to clipboard".into(),
                                        });
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            div()
                                .id("share-modal-dismiss")
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(13.0))
                                .child("Close")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_b.update(cx, |s, cx| {
                                        s.close_share();
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                // Reserved space for degraded events that fire before dismissal.
                .child(
                    div()
                        .min_h(px(24.0))
                        .mt_2()
                        .text_size(px(12.0))
                        .text_color(gpui::rgba(0xd97706ff)),
                ),
        )
        .into_any_element()
}

fn render_accept_modal(
    inline_error: Option<&str>,
    share_state: Entity<ShareUiState>,
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    window_handle: AnyWindowHandle,
    async_cx: AsyncApp,
    theme: OverlayTheme,
) -> AnyElement {
    let close_a = share_state.clone();
    let close_b = share_state.clone();
    let paste_state = share_state.clone();
    let inline_error_owned = inline_error.map(|s| s.to_string());

    overlay_backdrop("accept-modal")
        .child(
            modal_panel("accept-modal", 640.0, theme)
                .text_color(theme.fg)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .pb_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Accept share ticket"),
                        )
                        .child(
                            div()
                                .id("accept-modal-close")
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded(px(4.0))
                                .hover(|s| s.bg(gpui::rgba(0xffffff18)))
                                .child("✕")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_a.update(cx, |s, cx| {
                                        s.close_accept();
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                .child(div().text_size(px(13.0)).text_color(theme.muted_fg).child(
                    "Click 'Paste & accept' to read a ticket from the clipboard and attach \
                            the shared subtree under the currently focused block.",
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .id("accept-paste-button")
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .bg(gpui::rgba(0x2563ebff))
                                .text_color(gpui::rgba(0xffffffff))
                                .text_size(px(13.0))
                                .hover(|s| s.bg(gpui::rgba(0x1d4ed8ff)))
                                .child("Paste & accept")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    let clip = cx
                                        .read_from_clipboard()
                                        .and_then(|c| c.text().map(|s| s.to_string()))
                                        .unwrap_or_default();
                                    let focused = engine
                                        .focused_block()
                                        .map(|u| u.as_str().to_string())
                                        .unwrap_or_default();
                                    if clip.trim().is_empty() {
                                        paste_state.update(cx, |s, cx| {
                                            s.accept_error = Some(
                                                "clipboard is empty; copy a ticket first".into(),
                                            );
                                            cx.emit(NotifyShareUi);
                                            cx.notify();
                                        });
                                        return;
                                    }
                                    if focused.is_empty() {
                                        paste_state.update(cx, |s, cx| {
                                            s.accept_error = Some(
                                                "no focused block; click a parent block first"
                                                    .into(),
                                            );
                                            cx.emit(NotifyShareUi);
                                            cx.notify();
                                        });
                                        return;
                                    }
                                    dispatch_accept(
                                        session.clone(),
                                        rt_handle.clone(),
                                        paste_state.clone(),
                                        window_handle,
                                        &async_cx,
                                        focused,
                                        clip.trim().to_string(),
                                    );
                                }),
                        )
                        .child(
                            div()
                                .id("accept-modal-dismiss")
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(13.0))
                                .child("Close")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_b.update(cx, |s, cx| {
                                        s.close_accept();
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                .when_some(inline_error_owned, |this, e| {
                    this.child(
                        div()
                            .p_2()
                            .rounded(px(4.0))
                            .bg(gpui::rgba(0x80000030))
                            .text_size(px(12.0))
                            .text_color(gpui::rgba(0xfca5a5ff))
                            .child(format!("Error: {e}")),
                    )
                }),
        )
        .into_any_element()
}

fn render_quarantine_modal(
    idx: usize,
    q: &QuarantineEvent,
    share_state: Entity<ShareUiState>,
    theme: OverlayTheme,
) -> AnyElement {
    let shared_tree_id = q.shared_tree_id.clone();
    let quarantine_path = q.quarantine_path.clone();
    let quarantine_path_copy = quarantine_path.clone();

    let red_bg: Hsla = gpui::rgba(0x7f1d1dff).into();
    let red_border: Hsla = gpui::rgba(0xef4444ff).into();
    let red_theme = OverlayTheme {
        bg: red_bg,
        border: red_border,
        fg: gpui::rgba(0xffffffff).into(),
        muted_fg: gpui::rgba(0xfecacaff).into(),
    };

    let close_state = share_state.clone();

    overlay_backdrop(&format!("quarantine-{idx}"))
        .child(
            modal_panel(&format!("quarantine-{idx}"), 600.0, red_theme)
                .text_color(red_theme.fg)
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Share snapshot could not be restored"),
                )
                .child(div().text_size(px(13.0)).child(format!(
                    "Share `{shared_tree_id}` could not be restored. \
                        Your edits before the corruption are quarantined at \
                        `{quarantine_path}`. \
                        Re-accept the ticket from the other peer to restore."
                )))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .id(SharedString::from(format!("quarantine-dismiss-{idx}")))
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .bg(gpui::rgba(0xffffff1a))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(13.0))
                                .child("Dismiss")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    close_state.update(cx, |s, cx| {
                                        s.dismiss_quarantine(idx);
                                        cx.emit(NotifyShareUi);
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("quarantine-copy-path-{idx}")))
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded(px(6.0))
                                .bg(gpui::rgba(0xffffff1a))
                                .text_size(px(13.0))
                                .child("Copy path")
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        quarantine_path_copy.clone(),
                                    ));
                                }),
                        ),
                ),
        )
        .into_any_element()
}

fn render_toast_stack(
    toasts: &[DegradedToast],
    share_state: Entity<ShareUiState>,
    theme: OverlayTheme,
) -> AnyElement {
    let mut stack = div()
        .absolute()
        .bottom(px(16.0))
        .right(px(16.0))
        .flex()
        .flex_col()
        .gap_2();

    for (idx, toast) in toasts.iter().enumerate() {
        let (bg_color, icon, label) = match toast.kind {
            DegradedKind::SnapshotSaveFailed => {
                (gpui::rgba(0xfbbf24ff), "⚠", "Snapshot save failed")
            }
            DegradedKind::RehydrationFailed => (gpui::rgba(0xfbbf24ff), "↻", "Rehydration failed"),
            DegradedKind::Info => (gpui::rgba(0x60a5faff), "i", "Info"),
        };
        let close_state = share_state.clone();
        let msg = format!(
            "{icon}  {label} — {}",
            if toast.detail.len() > 80 {
                format!("{}…", &toast.detail[..80])
            } else {
                toast.detail.clone()
            }
        );
        stack = stack.child(
            div()
                .id(SharedString::from(format!("toast-{idx}")))
                .px_3()
                .py_2()
                .rounded(px(6.0))
                .bg(bg_color)
                .border_1()
                .border_color(theme.border)
                .text_color(gpui::rgba(0x000000cc))
                .text_size(px(12.0))
                .min_w(px(280.0))
                .max_w(px(420.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(div().child(msg))
                .child(
                    div()
                        .id(SharedString::from(format!("toast-close-{idx}")))
                        .cursor_pointer()
                        .pl_2()
                        .child("✕")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            close_state.update(cx, |s, cx| {
                                s.dismiss_toast(idx);
                                cx.emit(NotifyShareUi);
                                cx.notify();
                            });
                        }),
                ),
        );
    }

    stack.into_any_element()
}

fn render_error_modal(
    title: &str,
    message: &str,
    share_state: Entity<ShareUiState>,
    clear: fn(&mut ShareUiState),
    theme: OverlayTheme,
) -> AnyElement {
    let title = title.to_string();
    let message = message.to_string();
    let close_state = share_state.clone();

    overlay_backdrop("share-error-modal")
        .child(
            modal_panel("share-error", 520.0, theme)
                .text_color(theme.fg)
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .p_2()
                        .rounded(px(4.0))
                        .bg(gpui::rgba(0x80000030))
                        .text_color(gpui::rgba(0xfca5a5ff))
                        .text_size(px(12.0))
                        .child(message),
                )
                .child(
                    div()
                        .id("share-error-close")
                        .cursor_pointer()
                        .px_3()
                        .py_2()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(13.0))
                        .child("Close")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            close_state.update(cx, |s, cx| {
                                clear(s);
                                cx.emit(NotifyShareUi);
                                cx.notify();
                            });
                        }),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon::sync::{ShareDegraded, ShareDegradedReason};

    #[test]
    fn apply_degraded_routes_save_failed_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "abc".into(),
            reason: ShareDegradedReason::SnapshotSaveFailed("disk full".into()),
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::SnapshotSaveFailed);
        assert_eq!(s.toasts[0].shared_tree_id, "abc");
        assert!(s.quarantines.is_empty());
    }

    #[test]
    fn apply_degraded_routes_load_failed_to_quarantine() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "xyz".into(),
            reason: ShareDegradedReason::SnapshotLoadFailed("/tmp/x.corrupt-1".into()),
        });
        assert!(s.toasts.is_empty());
        assert_eq!(s.quarantines.len(), 1);
        assert_eq!(s.quarantines[0].quarantine_path, "/tmp/x.corrupt-1");
    }

    #[test]
    fn apply_degraded_routes_rehydration_failed_to_toast() {
        let mut s = ShareUiState::new();
        s.apply_degraded(ShareDegraded {
            shared_tree_id: "r".into(),
            reason: ShareDegradedReason::RehydrationFailed("endpoint".into()),
        });
        assert_eq!(s.toasts.len(), 1);
        assert_eq!(s.toasts[0].kind, DegradedKind::RehydrationFailed);
    }

    #[test]
    fn toast_stack_bounded_to_five() {
        let mut s = ShareUiState::new();
        for i in 0..8 {
            s.apply_degraded(ShareDegraded {
                shared_tree_id: format!("s{i}"),
                reason: ShareDegradedReason::SnapshotSaveFailed(format!("err{i}")),
            });
        }
        assert_eq!(s.toasts.len(), 5);
        // FIFO eviction: the first three were dropped.
        assert_eq!(s.toasts[0].shared_tree_id, "s3");
        assert_eq!(s.toasts[4].shared_tree_id, "s7");
    }

    #[test]
    fn ticket_parses_from_json_response() {
        let json = serde_json::json!({
            "ticket": "base64-ticket",
            "shared_tree_id": "share-1",
            "mount_block_id": "block:mount-1",
            "shared_root": "42:7",
        });
        let v = Value::String(json.to_string());
        let t = ShareTicket::from_value(&v).unwrap();
        assert_eq!(t.ticket, "base64-ticket");
        assert_eq!(t.shared_tree_id, "share-1");
        assert_eq!(t.mount_block_id, "block:mount-1");
    }

    #[test]
    fn ticket_parse_reports_missing_field() {
        let v = Value::String(r#"{"ticket":"x"}"#.to_string());
        let err = ShareTicket::from_value(&v).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("shared_tree_id"), "msg={msg}");
    }

    #[test]
    fn ticket_parse_rejects_non_string() {
        let v = Value::Integer(42);
        let err = ShareTicket::from_value(&v).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not a String"), "msg={msg}");
    }
}
