//! The Settings → Integrations list: one row per bundled integration, with a
//! switch and its configuration status.
//!
//! End users never run the enabling script, so this is the only surface on
//! which the enablement axis is reachable. The rows come from
//! [`IntegrationsSettingsVm`] rather than from anything GPUI owns, so the list
//! and its writes are the same on any frontend that grows one.
//!
//! Scope: this increment moves the STORED decision. Starting and stopping the
//! running MCP client fleet is not wired, so the section says so in place
//! rather than implying an effect the process does not perform.

use std::sync::Arc;

use gpui::AnyElement;
use gpui::Hsla;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::MouseButton;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::Styled;
use gpui::div;
use gpui::px;
use holon_app::integrations_settings::ConfigStatus;
use holon_app::integrations_settings::ConfigureProgress;
use holon_app::integrations_settings::IntegrationsSettingsVm;

use crate::geometry::BoundsRegistry;
use crate::geometry::TransparentTracker;
use crate::share_ui::DegradedKind;
use crate::share_ui::DegradedToast;
use crate::share_ui::DegradedToastSink;

/// GPUI global holding the settings list, installed in `main.rs` from the
/// DI-resolved view model — mirrors [`crate::share_ui::PendingWritesGlobal`].
#[derive(Clone)]
pub struct IntegrationsSettingsGlobal(pub Arc<IntegrationsSettingsVm>);

impl gpui::Global for IntegrationsSettingsGlobal {}

/// Tracked element id of `provider`'s switch — the handle a windowed test
/// clicks and the driver locates.
pub fn integration_toggle_id(provider: &str) -> String {
    format!("integration-toggle-{provider}")
}

/// Tracked element id of `provider`'s row label.
pub fn integration_row_id(provider: &str) -> String {
    format!("integration-row-{provider}")
}

/// Tracked element id of `provider`'s configuration status.
pub fn integration_status_id(provider: &str) -> String {
    format!("integration-status-{provider}")
}

/// Tracked element id of `provider`'s Configure button.
pub fn integration_configure_id(provider: &str) -> String {
    format!("integration-configure-{provider}")
}

/// Tracked element id of `provider`'s consent-flow progress line.
pub fn integration_progress_id(provider: &str) -> String {
    format!("integration-progress-{provider}")
}

/// The words on the button that starts the one-time consent flow.
pub const CONFIGURE_LABEL: &str = "Configure…";

/// Tracked element id of the next-launch disclosure under the section heading.
pub const NEXT_LAUNCH_NOTICE_ID: &str = "integrations-next-launch-notice";

/// Tracked element id of the disclosure [`render_unavailable`] paints.
pub const UNAVAILABLE_NOTICE_ID: &str = "integrations-unavailable-notice";

/// The words under the section heading. The switch stores a decision and does
/// not act on the running fleet, so the section has to say so; a silent
/// next-launch effect is the "silently degrades to look fine" case.
pub const NEXT_LAUNCH_NOTICE: &str = "Switching an integration on or off is saved immediately and takes effect at the next launch \
     — this does not start or stop a running integration.";

/// The words [`render_unavailable`] paints. A wiring bug must not read as an
/// empty list.
pub const UNAVAILABLE_NOTICE: &str = "Unavailable: this window has no integrations settings service. That is a wiring bug, not an \
     empty list — the switches below would be missing even for integrations that are switched on.";

/// Spawn the store-signal → window-refresh pump (mirrors
/// [`crate::oracles_ui::spawn_oracle_bridge`]).
///
/// The section reads `vm.rows()` on every render pass, so the only thing a
/// state change needs is a frame. Without this, a decision made anywhere other
/// than this window's own switch — a second window, an OAuth bootstrap, an
/// edited state file — would leave the switch showing the previous value until
/// something unrelated repainted.
pub fn spawn_integrations_bridge(
    vm: &Arc<IntegrationsSettingsVm>,
    rt_handle: &tokio::runtime::Handle,
    window_handle: gpui::AnyWindowHandle,
    async_cx: &gpui::AsyncApp,
) {
    use futures::StreamExt;
    use futures_signals::signal::SignalExt;
    use gpui::AppContext;

    let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
    for signal in vm.signals() {
        let tx = tx.clone();
        rt_handle.spawn(async move {
            // `to_stream` replays the current value first; that initial item is
            // one redundant frame at window open, not a missed change.
            let mut stream = signal.signal_cloned().to_stream();
            while stream.next().await.is_some() {
                if tx.unbounded_send(()).is_err() {
                    return; // pump gone
                }
            }
        });
    }
    // The consent flow runs off this window's thread and reports through its own
    // cells, so its progress needs a frame the same way a stored decision does.
    // Without this the status line would sit at "Waiting…" until something
    // unrelated repainted — including after the flow had already failed.
    for provider in vm.rows().into_iter().map(|r| r.provider) {
        let signal = vm.configure_progress(provider);
        let tx = tx.clone();
        rt_handle.spawn(async move {
            let mut stream = signal.signal_cloned().to_stream();
            while stream.next().await.is_some() {
                if tx.unbounded_send(()).is_err() {
                    return;
                }
            }
        });
    }

    async_cx
        .spawn(async move |cx| {
            while rx.next().await.is_some() {
                if let Err(e) = cx.update_window(window_handle, |_, window, _| window.refresh()) {
                    // A closed window is the ordinary end of this pump, and it
                    // fails identically to a genuine wiring break — so say it
                    // once and stop. Continuing would log one ERROR per state
                    // change for the rest of the process.
                    tracing::info!(
                        "integrations settings bridge stopping — its window no longer accepts \
                         updates: {e}"
                    );
                    return;
                }
            }
        })
        .detach();
}

/// Theme values the section paints with.
#[derive(Clone, Copy)]
pub struct SectionTheme {
    pub fg: Hsla,
    pub muted_fg: Hsla,
    pub border: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
}

/// Render the Integrations section for the Settings modal.
pub fn render_section(
    vm: Arc<IntegrationsSettingsVm>,
    theme: SectionTheme,
    bounds: BoundsRegistry,
) -> AnyElement {
    let mut section = div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .pt(px(12.0))
        .mt(px(12.0))
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme.fg)
                .child("Integrations"),
        )
        .child(
            TransparentTracker::new(
                NEXT_LAUNCH_NOTICE_ID.to_string(),
                "integration_notice",
                bounds.clone(),
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_fg)
                    .child(NEXT_LAUNCH_NOTICE)
                    .into_any_element(),
            )
            .with_displayed_text(NEXT_LAUNCH_NOTICE.to_string()),
        );

    for row in vm.rows() {
        section = section.child(render_row(
            &vm,
            row.provider,
            row.enabled,
            row.status,
            row.configurable,
            theme,
            &bounds,
        ));
    }

    section.into_any_element()
}

/// The Integrations section for the Settings modal, over whatever the window
/// holds. `None` means the view model never reached this window.
///
/// The branch lives here, not at the call site in `lib.rs`, so both arms are
/// reachable from a test: the fail-loud arm is the one nobody exercises by
/// hand, and a call-site `match` would leave it unreachable except by breaking
/// the DI wiring on purpose.
pub fn render_settings_integrations(
    settings: Option<&IntegrationsSettingsGlobal>,
    theme: SectionTheme,
    bounds: BoundsRegistry,
) -> AnyElement {
    match settings {
        Some(g) => render_section(g.0.clone(), theme, bounds),
        None => render_unavailable(theme, bounds),
    }
}

/// The section as it renders when the view model never reached the window.
///
/// A wiring bug here would otherwise leave the Settings modal looking like a
/// build that ships no integrations at all — indistinguishable, from the user's
/// side, from the empty list.
pub fn render_unavailable(theme: SectionTheme, bounds: BoundsRegistry) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .pt(px(12.0))
        .mt(px(12.0))
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme.fg)
                .child("Integrations"),
        )
        .child(
            TransparentTracker::new(
                UNAVAILABLE_NOTICE_ID.to_string(),
                "integration_notice",
                bounds,
                div()
                    .text_size(px(11.0))
                    .text_color(theme.danger)
                    .child(UNAVAILABLE_NOTICE)
                    .into_any_element(),
            )
            .with_displayed_text(UNAVAILABLE_NOTICE.to_string()),
        )
        .into_any_element()
}

fn render_row(
    vm: &Arc<IntegrationsSettingsVm>,
    provider: &'static str,
    enabled: bool,
    status: ConfigStatus,
    configurable: bool,
    theme: SectionTheme,
    bounds: &BoundsRegistry,
) -> AnyElement {
    let label = TransparentTracker::new(
        integration_row_id(provider),
        "integration_row",
        bounds.clone(),
        div()
            .text_size(px(12.0))
            .text_color(theme.fg)
            .child(provider)
            .into_any_element(),
    )
    .with_displayed_text(provider.to_string());

    let status_el = TransparentTracker::new(
        integration_status_id(provider),
        "integration_status",
        bounds.clone(),
        div()
            .text_size(px(11.0))
            .text_color(theme.muted_fg)
            .child(status.label())
            .into_any_element(),
    )
    .with_displayed_text(status.label().to_string());

    let mut details = div()
        .flex()
        .flex_col()
        .flex_1()
        .child(label)
        .child(status_el);

    let progress = vm.configure_progress(provider).get_cloned();
    if let Some(message) = progress.message() {
        details = details.child(
            TransparentTracker::new(
                integration_progress_id(provider),
                "integration_progress",
                bounds.clone(),
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_fg)
                    .child(message.clone())
                    .into_any_element(),
            )
            .with_displayed_text(message),
        );
    }

    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .py(px(4.0))
        .child(details);

    // Only an unconfigured integration that HAS a consent flow gets the button:
    // re-running consent for a configured one would replace a working refresh
    // token that some providers will not mint twice without a manual revoke,
    // and an integration with no OAuth2 arm has nothing to configure at all.
    //
    // It also goes away while a flow is running. The view model refuses a second
    // flow anyway, but a button that stays clickable and silently does nothing
    // reads as broken — withdrawing it is how the refusal becomes visible.
    let in_flight = progress == ConfigureProgress::AwaitingConsent;
    if status == ConfigStatus::Unconfigured && configurable && !in_flight {
        row = row.child(render_configure_button(vm, provider, theme, bounds));
    }

    row.child(render_switch(vm, provider, enabled, theme, bounds))
        .into_any_element()
}

/// The button that starts `provider`'s one-time consent flow.
///
/// The flow is minutes long (it waits for a human in a browser) and needs a
/// tokio reactor for its loopback listener and its HTTPS exchange, so it runs
/// on a thread and a runtime of its own rather than occupying the app's. A
/// one-shot, user-initiated flow is exactly the case where that isolation is
/// worth more than sharing the executor.
fn render_configure_button(
    vm: &Arc<IntegrationsSettingsVm>,
    provider: &'static str,
    theme: SectionTheme,
    bounds: &BoundsRegistry,
) -> AnyElement {
    let el_id = integration_configure_id(provider);
    let vm = vm.clone();

    let clickable = div()
        .id(SharedString::from(el_id.clone()))
        .cursor_pointer()
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(theme.border)
        .text_size(px(11.0))
        .text_color(theme.fg)
        .child(CONFIGURE_LABEL)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let vm = vm.clone();
            let started = std::thread::Builder::new()
                .name(format!("holon-oauth-{provider}"))
                .spawn(move || {
                    let runtime = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            // The VM's progress cell is the user-visible channel
                            // and it is unreachable without a runtime, so this
                            // one failure has to speak for itself in the log.
                            tracing::error!(
                                provider,
                                "could not start a runtime for the OAuth consent flow: {e}"
                            );
                            return;
                        }
                    };
                    // The result also lands in the progress cell the row renders;
                    // it is dropped here rather than swallowed.
                    let _ = runtime.block_on(vm.configure_with_system_browser(provider));
                });

            if let Err(e) = started {
                DegradedToastSink::push(
                    DegradedToast {
                        kind: DegradedKind::CommandFailed,
                        shared_tree_id: provider.to_string(),
                        detail: format!("Could not start the consent flow for '{provider}': {e}"),
                        condition: None,
                    },
                    cx,
                );
            }
            window.refresh();
        });

    TransparentTracker::new(
        el_id,
        "integration_configure",
        bounds.clone(),
        clickable.into_any_element(),
    )
    .with_displayed_text(CONFIGURE_LABEL.to_string())
    .into_any_element()
}

fn render_switch(
    vm: &Arc<IntegrationsSettingsVm>,
    provider: &'static str,
    enabled: bool,
    theme: SectionTheme,
    bounds: &BoundsRegistry,
) -> AnyElement {
    // Same track/knob geometry as the preferences toggle (`pref_field.rs`), so
    // the two switch kinds in one Settings modal read as one control.
    let (track_bg, knob_offset) = if enabled {
        (theme.success, px(18.0))
    } else {
        (gpui::hsla(0.0, 0.0, 1.0, 0.2), px(2.0))
    };
    let track = div()
        .w(px(36.0))
        .h(px(20.0))
        .rounded(px(10.0))
        .bg(track_bg)
        .relative()
        .child(
            div()
                .absolute()
                .top(px(2.0))
                .left(knob_offset)
                .w(px(16.0))
                .h(px(16.0))
                .rounded(px(8.0))
                .bg(gpui::rgba(0xffffffee)),
        );

    let el_id = integration_toggle_id(provider);
    let vm = vm.clone();
    let clickable = div()
        .id(SharedString::from(el_id.clone()))
        .cursor_pointer()
        .child(track)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if let Err(e) = vm.set_enabled(provider, !enabled) {
                DegradedToastSink::push(
                    DegradedToast {
                        kind: DegradedKind::CommandFailed,
                        shared_tree_id: provider.to_string(),
                        detail: format!("Could not switch '{provider}': {e:#}"),
                        condition: None,
                    },
                    cx,
                );
            }
            window.refresh();
        });

    TransparentTracker::new(
        el_id,
        "integration_toggle",
        bounds.clone(),
        clickable.into_any_element(),
    )
    // Bounds alone cannot tell an on switch from an off one, so the state
    // travels with the element.
    .with_displayed_text(if enabled { "on" } else { "off" }.to_string())
    .into_any_element()
}
