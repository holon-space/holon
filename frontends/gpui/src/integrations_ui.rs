//! What is LEFT of the Settings → Integrations section after D5.b.
//!
//! The list itself — one row per bundled integration, with its statuses and its
//! enablement switch — is layout data now
//! (`holon_app::integrations_section`), rendered through the same pipeline as
//! every other panel and writing through `integration.set_field`. None of it
//! lives here any more.
//!
//! What remains is the one affordance that is NOT a field write: the OAuth
//! consent flow. `configure` opens a browser, waits minutes for a human, and
//! reports through its own progress cells — a long-running side effect with no
//! `set_field` shape. It stays a native strip beneath the layout-data list
//! until it has an operation of its own (design §6: the `integration` provider
//! gains a `begin_oauth` descriptor, and the strip becomes an `op_button`).

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

/// Tracked element id of the disclosure [`render_unavailable`] paints.
pub const UNAVAILABLE_NOTICE_ID: &str = "integrations-unavailable-notice";

/// The words [`render_unavailable`] paints. A wiring bug must not read as
/// "nothing needs configuring".
pub const UNAVAILABLE_NOTICE: &str = "Unavailable: this window has no integrations settings service. That is a wiring bug, not a \
     fully-configured vault — the setup buttons below would be missing even for integrations \
     that still need credentials.";

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

/// The Settings modal's residual native strip: the consent flows that are still
/// waiting for a human, plus whatever the last one had to say.
///
/// One row per integration that has something to offer — an unconfigured
/// provider whose sidecar declares an OAuth2 arm, or any provider with a
/// progress message. A provider with neither draws nothing: its whole row is
/// already in the layout-data list above, and repeating it here would give the
/// modal two rows per integration.
pub fn render_configure_strip(
    vm: Arc<IntegrationsSettingsVm>,
    theme: SectionTheme,
    bounds: BoundsRegistry,
) -> AnyElement {
    let mut strip = div().flex().flex_col().gap(px(6.0));
    let mut drew_any = false;

    for row in vm.rows() {
        let progress = vm.configure_progress(row.provider).get_cloned();
        let message = progress.message();
        // Only an unconfigured integration that HAS a consent flow gets the
        // button: re-running consent for a configured one would replace a
        // working refresh token that some providers will not mint twice without
        // a manual revoke, and an integration with no OAuth2 arm has nothing to
        // configure at all.
        //
        // It also goes away while a flow is running. The view model refuses a
        // second flow anyway, but a button that stays clickable and silently
        // does nothing reads as broken — withdrawing it is how the refusal
        // becomes visible.
        let in_flight = progress == ConfigureProgress::AwaitingConsent;
        let offers_setup =
            row.status == ConfigStatus::Unconfigured && row.configurable && !in_flight;
        if !offers_setup && message.is_none() {
            continue;
        }
        drew_any = true;

        let mut line = div().flex().flex_row().items_center().gap(px(8.0)).child(
            div()
                .text_size(px(12.0))
                .text_color(theme.fg)
                .child(SharedString::from(row.provider)),
        );
        if let Some(message) = message {
            line = line.child(
                TransparentTracker::new(
                    integration_progress_id(row.provider),
                    "integration_progress",
                    bounds.clone(),
                    div()
                        .flex_1()
                        .text_size(px(11.0))
                        .text_color(theme.muted_fg)
                        .child(message.clone())
                        .into_any_element(),
                )
                .with_displayed_text(message),
            );
        }
        if offers_setup {
            line = line.child(render_configure_button(&vm, row.provider, theme, &bounds));
        }
        strip = strip.child(line);
    }

    if !drew_any {
        return div().into_any_element();
    }
    strip.pt(px(8.0)).into_any_element()
}

/// The Settings modal's Configure strip, over whatever the window holds.
/// `None` means the view model never reached this window.
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
        Some(g) => render_configure_strip(g.0.clone(), theme, bounds),
        None => render_unavailable(theme, bounds),
    }
}

/// The strip as it renders when the view model never reached the window.
///
/// A wiring bug here would otherwise leave the Settings modal looking like a
/// vault whose integrations are all set up — indistinguishable, from the
/// user's side, from having nothing left to configure.
pub fn render_unavailable(theme: SectionTheme, bounds: BoundsRegistry) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .pt(px(8.0))
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
