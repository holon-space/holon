//! Windowed rung for the Settings modal's residual OAuth strip.
//!
//! The integrations LIST is layout data now — one query, one entity profile,
//! one `state_toggle` per row — so what it paints and what a click on its
//! switch does are covered by `holon-app::integration_state_projection`,
//! `holon-app::integration_set_field_op` and
//! `holon-gpui::state_toggle_switch_windowed`. Nothing about rows, statuses or
//! switches is asserted here any more.
//!
//! What is left is the one affordance with no `set_field` shape: the one-time
//! consent flow. It stays native (see `integrations_ui`), and only a window
//! shows whether its button appears on the right rows, withdraws while a flow
//! is running, and reports a failure the user can act on.
//!
//! What this rung does NOT cover: the signal → frame pump
//! (`spawn_integrations_bridge`). It needs a live tokio runtime whose wakeups
//! gpui's test scheduler does not drive, so
//! `a_failed_consent_flow_paints_its_reason` is deliberately written pump-free
//! — it drives the flow to completion, asks for a frame explicitly, and asserts
//! what the frame painted. What stays unproven here is only the *automatic*
//! repaint. On the real app that is the pump's job, and it is verified by hand
//! (see the lane report), not here.
//!
//! Run: cargo test -p holon-gpui --test integrations_settings_windowed

use std::sync::Arc;

use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::prelude::*;
use holon_app::integrations_settings::ConfigureProgress;
use holon_app::integrations_settings::IntegrationsSettingsVm;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::integrations_ui::CONFIGURE_LABEL;
use holon_gpui::integrations_ui::IntegrationsSettingsGlobal;
use holon_gpui::integrations_ui::SectionTheme;
use holon_gpui::integrations_ui::UNAVAILABLE_NOTICE;
use holon_gpui::integrations_ui::UNAVAILABLE_NOTICE_ID;
use holon_gpui::integrations_ui::integration_configure_id;
use holon_gpui::integrations_ui::integration_progress_id;
use holon_gpui::integrations_ui::render_settings_integrations;

/// The providers this build ships, in bundle order — spelled out so the test
/// states what the user must see rather than echoing the bundle.
const BUNDLED: &[&str] = &[
    "claude-history",
    "gcal",
    "gmail",
    "jsonplaceholder",
    "todoist",
];

struct Fixture {
    /// `None` reproduces a window the view model never reached.
    settings: Option<IntegrationsSettingsGlobal>,
    bounds: BoundsRegistry,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        // Through the PRODUCTION branch, not around it: `lib.rs` hands the
        // global straight to this function, so an absent view model reaches the
        // fail-loud arm here exactly as it would in the app.
        gpui::div().size_full().child(render_settings_integrations(
            self.settings.as_ref(),
            theme(),
            self.bounds.clone(),
        ))
    }
}

fn theme() -> SectionTheme {
    SectionTheme {
        fg: gpui::hsla(0.0, 0.0, 0.9, 1.0),
        muted_fg: gpui::hsla(0.0, 0.0, 0.6, 1.0),
        border: gpui::hsla(0.0, 0.0, 0.3, 1.0),
        success: gpui::hsla(0.33, 0.6, 0.45, 1.0),
        danger: gpui::hsla(0.0, 0.7, 0.5, 1.0),
    }
}

struct Rig {
    /// The SAME view model the window renders from. `None` in the
    /// no-view-model rig.
    vm: Option<Arc<IntegrationsSettingsVm>>,
    bounds: BoundsRegistry,
    /// The integrations directory the store reads and writes.
    dir: tempfile::TempDir,
}

impl Rig {
    fn vm(&self) -> &Arc<IntegrationsSettingsVm> {
        self.vm.as_ref().expect("this rig has a view model")
    }
}

fn mount(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let vm = Arc::new(IntegrationsSettingsVm::over_dir(dir.path()).expect("settings list loads"));
    let settings = Some(IntegrationsSettingsGlobal(vm.clone()));
    let (bounds, vcx) = open(cx, settings);
    (
        Rig {
            vm: Some(vm),
            bounds,
            dir,
        },
        vcx,
    )
}

/// A rig whose `provider` was already through a consent flow.
///
/// The state file is written BEFORE the view model loads, because a second
/// store over the same directory shares the files but not the signals — see
/// `a_write_from_a_second_store_is_durable_but_unseen_until_relaunch`.
fn mount_configured<'a>(
    cx: &'a mut TestAppContext,
    provider: &str,
) -> (Rig, &'a mut VisualTestContext) {
    use holon_mcp_client::IntegrationConfigStore;
    use holon_mcp_client::integration_state::Configuration;
    use holon_mcp_client::integration_state::CredentialRef;
    use holon_mcp_client::integration_state::Credentials;
    use holon_mcp_client::integration_state::IntegrationState;

    let dir = tempfile::tempdir().expect("tempdir");
    let seeding = IntegrationConfigStore::load(dir.path()).expect("a store to seed with");
    seeding
        .set(
            provider,
            IntegrationState {
                enabled: true,
                configuration: Configuration::Configured(Credentials {
                    client_id: CredentialRef::File {
                        path: dir.path().join("client-id"),
                    },
                    client_secret: CredentialRef::File {
                        path: dir.path().join("client-secret"),
                    },
                    refresh_token_file: dir.path().join("refresh-token"),
                }),
            },
        )
        .expect("seed a configured provider");
    drop(seeding);

    let vm = Arc::new(IntegrationsSettingsVm::over_dir(dir.path()).expect("settings list loads"));
    let settings = Some(IntegrationsSettingsGlobal(vm.clone()));
    let (bounds, vcx) = open(cx, settings);
    (
        Rig {
            vm: Some(vm),
            bounds,
            dir,
        },
        vcx,
    )
}

/// A rig whose `provider` sidecar is an installed override pointing at
/// credential paths INSIDE the rig's tempdir.
///
/// Without this, a consent-flow test reads the bundled sidecar's
/// `~/.config/holon/...` paths — the developer's real credentials — and the
/// test's behaviour depends on whose machine it runs on.
fn mount_with_sandboxed_oauth<'a>(
    cx: &'a mut TestAppContext,
    provider: &str,
) -> (Rig, &'a mut VisualTestContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundled = holon_mcp_client::bundled_sidecars::bundled_sidecar(provider)
        .expect("the provider under test is bundled");
    let sandboxed = bundled.yaml.replace(
        "~/.config/holon/",
        &format!("{}/", dir.path().join("creds").display()),
    );
    assert_ne!(
        sandboxed, bundled.yaml,
        "the sandbox must actually redirect the sidecar's credential paths"
    );
    std::fs::write(dir.path().join(format!("{provider}.yaml")), &sandboxed)
        .expect("install the sandboxed sidecar");

    let vm = Arc::new(IntegrationsSettingsVm::over_dir(dir.path()).expect("settings list loads"));
    let settings = Some(IntegrationsSettingsGlobal(vm.clone()));
    let (bounds, vcx) = open(cx, settings);
    (
        Rig {
            vm: Some(vm),
            bounds,
            dir,
        },
        vcx,
    )
}

/// A window the view model never reached — the wiring-bug case.
fn mount_without_settings(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (bounds, vcx) = open(cx, None);
    (
        Rig {
            vm: None,
            bounds,
            dir,
        },
        vcx,
    )
}

fn open(
    cx: &mut TestAppContext,
    settings: Option<IntegrationsSettingsGlobal>,
) -> (BoundsRegistry, &mut VisualTestContext) {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();
    let (_root, vcx) = cx.add_window_view(move |window, cx| {
        let fixture = cx.new(|_cx| Fixture {
            settings,
            bounds: bounds_for_view,
        });
        gpui_component::Root::new(fixture, window, cx)
    });
    (bounds, vcx)
}

fn element(rig: &Rig, el_id: &str) -> Option<ElementInfo> {
    rig.bounds.flush();
    rig.bounds
        .all_elements()
        .into_iter()
        .find(|(id, _)| id == el_id)
        .map(|(_, i)| i)
}

fn painted_text(rig: &Rig, el_id: &str) -> Option<String> {
    element(rig, el_id).and_then(|i| i.displayed_text.map(|t| t.to_string()))
}

/// The fail-loud arm. Reached through the same `render_settings_integrations`
/// branch production uses, with no view model in the window.
#[gpui::test]
fn a_window_without_the_view_model_says_so_instead_of_painting_an_empty_list(
    cx: &mut TestAppContext,
) {
    let (rig, vcx) = mount_without_settings(cx);
    vcx.run_until_parked();

    let painted = painted_text(&rig, UNAVAILABLE_NOTICE_ID);
    assert_eq!(
        painted.as_deref(),
        Some(UNAVAILABLE_NOTICE),
        "a window that never got the settings list must SAY the section is \
         unavailable — a silent empty section is indistinguishable from a build \
         that ships no integrations, which is the one thing this must never look \
         like"
    );

    for provider in BUNDLED {
        assert_eq!(
            painted_text(&rig, &integration_configure_id(provider)),
            None,
            "'{provider}' must paint NO setup button in the unavailable arm — a \
             button with no view model behind it would silently drop the user's click"
        );
    }
}

/// Providers whose sidecar declares an OAuth2 consent flow. The other bundled
/// providers authenticate with a static token or none at all, so a Configure
/// button on their rows would be a dead end.
const OAUTH_BUNDLED: &[&str] = &["gcal", "gmail"];

/// An unconfigured OAuth integration must offer the in-app consent flow, and a
/// non-OAuth one must not. Before this affordance existed the only route was a
/// shell script, which is not a route an end user has.
#[gpui::test]
fn only_unconfigured_oauth_integrations_paint_a_configure_button(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    vcx.run_until_parked();

    for provider in BUNDLED {
        let painted = painted_text(&rig, &integration_configure_id(provider));
        if OAUTH_BUNDLED.contains(provider) {
            assert_eq!(
                painted.as_deref(),
                Some(CONFIGURE_LABEL),
                "'{provider}' is an unconfigured OAuth integration, so the row must \
                 offer the one-time setup — without it the only route is a shell script"
            );
        } else {
            assert_eq!(
                painted, None,
                "'{provider}' has no OAuth2 consent flow, so a Configure button on its \
                 row would be a control that cannot do anything"
            );
        }
    }
}

/// A configured integration must NOT offer it. Re-running consent replaces a
/// working refresh token, and providers commonly refuse to mint a second one
/// without a manual revoke first — so an idle click could break a working
/// integration with no way back inside the app.
#[gpui::test]
fn a_configured_integration_does_not_paint_a_configure_button(cx: &mut TestAppContext) {
    let (rig, vcx) = mount_configured(cx, "gcal");
    vcx.run_until_parked();

    assert_eq!(
        rig.vm()
            .rows()
            .into_iter()
            .find(|r| r.provider == "gcal")
            .map(|r| r.status),
        Some(holon_app::integrations_settings::ConfigStatus::Configured),
        "precondition: the rig really did seed a configured gcal"
    );
    assert!(
        element(&rig, &integration_configure_id("gcal")).is_none(),
        "a configured integration must not offer to re-run consent"
    );
    assert_eq!(
        painted_text(&rig, &integration_configure_id("gmail")).as_deref(),
        Some(CONFIGURE_LABEL),
        "…while its unconfigured neighbour still does"
    );
}

/// A browser that records instead of launching. A windowed test must never open
/// the developer's real browser at a real consent page.
struct NoBrowser(std::sync::Mutex<usize>);

impl holon_mcp_client::oauth_bootstrap::BrowserOpener for NoBrowser {
    fn open(&self, _: &str) -> anyhow::Result<()> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
}

/// A consent flow that fails must SAY so on the row.
///
/// This is the failure mode the whole affordance lives or dies by: the flow
/// runs off this window's thread, so without a rendered progress line a failure
/// is indistinguishable from a button that does nothing at all.
///
/// Hermetic on two axes that both bit during development: the browser is a
/// recording stub, and the sidecar under test is an installed override whose
/// credential paths sit in the rig's tempdir. Against the BUNDLED sidecar this
/// test read `~/.config/holon/gcal-client-id` and, on a developer machine that
/// has one, opened a real Google consent page.
///
/// The flow is driven on a runtime of the test's own: it needs a reactor, and
/// gpui's test scheduler does not drive tokio wakeups.
#[gpui::test]
fn a_failed_consent_flow_paints_its_reason_on_the_row(cx: &mut TestAppContext) {
    let (rig, vcx) = mount_with_sandboxed_oauth(cx, "gcal");
    vcx.run_until_parked();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime for the flow under test");
    let browser = NoBrowser(std::sync::Mutex::new(0));
    let outcome = runtime.block_on(rig.vm().configure(
        "gcal",
        &browser,
        std::time::Duration::from_millis(200),
    ));
    assert!(
        outcome.is_err(),
        "precondition: the sandboxed sidecar points at credential files that do not \
         exist, so the flow must refuse"
    );
    assert_eq!(
        *browser.0.lock().unwrap(),
        0,
        "the flow must refuse BEFORE the browser hop — a consent page opened for a \
         setup that cannot be stored wastes a consent some providers issue once"
    );

    vcx.update(|window, _cx| window.refresh());
    vcx.run_until_parked();

    let painted = painted_text(&rig, &integration_progress_id("gcal")).unwrap_or_else(|| {
        panic!(
            "a failed consent flow must paint its reason on the row — a silent \
             failure is indistinguishable from a dead button"
        )
    });
    assert!(
        painted.starts_with("Configuration failed:"),
        "the line must read as a failure, got: {painted}"
    );
    assert!(
        painted.contains("client_id") || painted.contains("client-id"),
        "the line must name what is missing so the user can fix it, got: {painted}"
    );
    assert!(
        element(&rig, &integration_progress_id("gmail")).is_none(),
        "one flow must annotate ONE row"
    );
}

// ---------------------------------------------------------------------------
// Round 2 — D6: the in-flight guard, windowed.
// ---------------------------------------------------------------------------

/// D6. While a consent flow is running, the row must stop offering to start
/// another one.
///
/// The button kept painting for the whole flow (the status axis does not change
/// until it finishes), so every extra click spawned another browser hop,
/// another loopback listener and another writer racing the same token file —
/// all reporting through one shared progress cell, so the last one to finish
/// decided what the user was told.
///
/// Driven through the real flow: a sandboxed sidecar with provisioned client
/// credentials gets past resolution and then parks in the loopback wait, which
/// is exactly the in-flight state a user sees while their browser is open.
#[gpui::test]
fn an_in_flight_consent_flow_withdraws_the_configure_button(cx: &mut TestAppContext) {
    let (rig, vcx) = mount_with_sandboxed_oauth(cx, "gcal");
    vcx.run_until_parked();

    assert_eq!(
        painted_text(&rig, &integration_configure_id("gcal")).as_deref(),
        Some(CONFIGURE_LABEL),
        "precondition: an idle unconfigured row offers the setup"
    );

    // Provision the client credentials the sandboxed sidecar points at, so the
    // flow reaches the loopback wait instead of refusing early.
    let creds = rig.dir.path().join("creds");
    std::fs::create_dir_all(&creds).expect("creds dir");
    for (name, value) in [
        ("gcal-client-id", "sandbox-client-id.apps.example.com"),
        ("gcal-client-secret", "sandbox-client-secret"),
    ] {
        let path = creds.join(name);
        std::fs::write(&path, value).expect("write credential");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    // Park a real flow in its loopback wait on a runtime of its own. The window
    // is answered from this thread; nextest gives each test its own process, so
    // the parked flow dies with it.
    let vm = rig.vm().clone();
    let progress = vm.configure_progress("gcal");
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime for the parked flow");
        let _ = rt.block_on(vm.configure(
            "gcal",
            &NoBrowser(std::sync::Mutex::new(0)),
            std::time::Duration::from_secs(120),
        ));
    });

    // Wait for the flow to actually reach the in-flight state.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while progress.get_cloned() != ConfigureProgress::AwaitingConsent {
        assert!(
            std::time::Instant::now() < deadline,
            "the flow never reached AwaitingConsent; it is {:?}",
            progress.get_cloned()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    vcx.update(|window, _cx| window.refresh());
    vcx.run_until_parked();

    assert!(
        element(&rig, &integration_configure_id("gcal")).is_none(),
        "the row must withdraw the Configure button while a flow is in flight — a button that \
         stays clickable and silently starts nothing reads as broken, and before the guard it \
         started a SECOND browser hop and a second writer on the same token file"
    );
    assert_eq!(
        painted_text(&rig, &integration_progress_id("gcal")).as_deref(),
        Some("Waiting for you to finish in the browser — Holon is listening for the redirect."),
        "…and the row must say why the button is gone"
    );
    assert_eq!(
        painted_text(&rig, &integration_configure_id("gmail")).as_deref(),
        Some(CONFIGURE_LABEL),
        "one flow must not withdraw another provider's button"
    );
}
