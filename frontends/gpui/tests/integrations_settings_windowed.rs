//! Windowed rung for Settings → Integrations.
//!
//! End users never run the enabling script, so this section is the only place
//! an integration can be switched on. The keystone cannot see it (it is a
//! window-chrome surface, not block layout), and the headless view-model rung
//! (`holon-app::integrations_settings_vm`) cannot see whether any of it reaches
//! a frame. This rung covers exactly the part that only a window shows:
//!
//!  - every bundled integration paints a row, a status and a switch, and the
//!    switch's painted state matches the store;
//!  - clicking a switch persists through the store AND repaints — a dead switch
//!    and a live one are indistinguishable from the store's tests alone;
//!  - the painted state is READ from the store on every pass, not captured when
//!    the modal opened, which is what lets a change made elsewhere show up.
//!
//! What this rung does NOT cover: the signal → frame pump
//! (`spawn_integrations_bridge`). It needs a live tokio runtime whose wakeups
//! gpui's test scheduler does not drive; the third case below asserts the half
//! that survives without it — that a repaint shows the store's current value.
//!
//! Run: cargo test -p holon-gpui --test integrations_settings_windowed

use std::sync::Arc;

use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::prelude::*;
use gpui::px;
use holon_app::integrations_settings::IntegrationsSettingsVm;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::integrations_ui::IntegrationsSettingsGlobal;
use holon_gpui::integrations_ui::NEXT_LAUNCH_NOTICE;
use holon_gpui::integrations_ui::NEXT_LAUNCH_NOTICE_ID;
use holon_gpui::integrations_ui::SectionTheme;
use holon_gpui::integrations_ui::UNAVAILABLE_NOTICE;
use holon_gpui::integrations_ui::UNAVAILABLE_NOTICE_ID;
use holon_gpui::integrations_ui::integration_row_id;
use holon_gpui::integrations_ui::integration_status_id;
use holon_gpui::integrations_ui::integration_toggle_id;
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

fn center(info: &ElementInfo) -> gpui::Point<gpui::Pixels> {
    gpui::point(
        px(info.x + info.width / 2.0),
        px(info.y + info.height / 2.0),
    )
}

#[gpui::test]
fn every_bundled_integration_paints_a_row_a_status_and_a_switch(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    vcx.run_until_parked();

    for provider in BUNDLED {
        assert_eq!(
            painted_text(&rig, &integration_row_id(provider)).as_deref(),
            Some(*provider),
            "'{provider}' is bundled, so the settings list must name it — a list \
             built from what LOADED would hide exactly the integrations the user \
             opened Settings to switch on"
        );
        assert_eq!(
            painted_text(&rig, &integration_status_id(provider)).as_deref(),
            Some("Unconfigured"),
            "'{provider}' has no credentials in a clean vault"
        );
        assert_eq!(
            painted_text(&rig, &integration_toggle_id(provider)).as_deref(),
            Some("off"),
            "'{provider}' has no state file, so its switch must paint as off"
        );
    }
}

/// The switch stores a decision the running process does not act on, so the
/// section owes the user that sentence. An undisclosed next-launch effect is
/// the "silently degrades to look fine" case: the user flips a switch, nothing
/// happens, and nothing on screen explains why.
#[gpui::test]
fn the_section_paints_the_next_launch_disclosure(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    vcx.run_until_parked();

    let painted = painted_text(&rig, NEXT_LAUNCH_NOTICE_ID);
    assert_eq!(
        painted.as_deref(),
        Some(NEXT_LAUNCH_NOTICE),
        "the section must paint the next-launch disclosure verbatim — deleting it \
         leaves a switch that silently does less than it appears to"
    );
    let notice = painted.expect("checked above");
    assert!(
        notice.contains("next launch") && notice.contains("does not start or stop"),
        "the disclosure must say BOTH what does happen and what does not, got: {notice}"
    );
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
            painted_text(&rig, &integration_toggle_id(provider)),
            None,
            "'{provider}' must paint NO switch in the unavailable arm — a switch \
             with no store behind it would silently drop the user's click"
        );
    }
}

#[gpui::test]
fn clicking_a_switch_persists_the_decision_and_repaints_it(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    vcx.run_until_parked();

    let toggle = element(&rig, &integration_toggle_id("todoist"))
        .expect("precondition: todoist paints a switch");
    let at = center(&toggle);
    vcx.simulate_mouse_move(at, None, Default::default());
    vcx.simulate_click(at, Default::default());
    vcx.run_until_parked();

    let path = rig.vm().state_path("todoist").expect("state path");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "clicking the switch must write '{}', but: {e}. A switch that paints \
             but stores nothing is the one failure the store's own tests cannot see",
            path.display()
        )
    });
    assert!(
        text.contains("enabled = true"),
        "the click must record the decision, got:\n{text}"
    );

    assert_eq!(
        painted_text(&rig, &integration_toggle_id("todoist")).as_deref(),
        Some("on"),
        "the switch must repaint in its new position — a switch that stores the \
         decision but keeps painting the old one reads as a dead control"
    );
    assert_eq!(
        painted_text(&rig, &integration_toggle_id("gcal")).as_deref(),
        Some("off"),
        "one click must move ONE integration"
    );
}

/// The section re-reads `rows()` every pass instead of capturing a list when
/// the modal opened. This is what makes the signal → frame pump sufficient: the
/// pump only has to ask for a frame.
///
/// The write here comes from the window's OWN view model, not from another
/// process — it is the re-read that is under test, not externality. The
/// cross-store boundary is the next rung.
#[gpui::test]
fn the_section_rereads_the_rows_on_every_pass(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    vcx.run_until_parked();

    rig.vm()
        .set_enabled("gmail", true)
        .expect("enable through the window's own view model");

    vcx.update(|window, _cx| window.refresh());
    vcx.run_until_parked();

    assert_eq!(
        painted_text(&rig, &integration_toggle_id("gmail")).as_deref(),
        Some("on"),
        "the painted switch must follow the store on every pass — a list captured \
         when the modal opened would still read 'off'"
    );
}

/// THE BOUNDARY of this increment, pinned rather than implied away.
///
/// A second store over the same directory shares the FILES but not the
/// `Mutable` cells, and nothing watches the directory. So a decision written by
/// another process — a second window, an external OAuth bootstrap, a
/// hand-edited state file — is durable on disk and invisible to this window
/// until the next launch.
///
/// This test asserts that gap on purpose. It will go red the day a file watcher
/// or a shared-store handle lands, and that red is the correct signal: the
/// increment's boundary moved and this rung should become the external-write
/// test the name `a_repaint_shows_a_decision_this_window_did_not_make`
/// promised.
#[gpui::test]
fn a_write_from_a_second_store_is_durable_but_unseen_until_relaunch(cx: &mut TestAppContext) {
    let (rig, vcx) = mount(cx);
    vcx.run_until_parked();

    // Another process's view of the same integrations directory.
    let elsewhere =
        IntegrationsSettingsVm::over_dir(rig.dir.path()).expect("second store over the same dir");
    elsewhere
        .set_enabled("gmail", true)
        .expect("external process enables gmail");

    let path = rig.vm().state_path("gmail").expect("state path");
    let text = std::fs::read_to_string(&path).expect("the external write reached disk");
    assert!(
        text.contains("enabled = true"),
        "precondition: the other process really did persist the decision, got:\n{text}"
    );

    vcx.update(|window, _cx| window.refresh());
    vcx.run_until_parked();

    assert_eq!(
        painted_text(&rig, &integration_toggle_id("gmail")).as_deref(),
        Some("off"),
        "KNOWN BOUNDARY of this increment: nothing watches the integrations \
         directory, so a write by another process stays invisible to this window \
         until relaunch. If this assertion fails, a watcher landed — replace this \
         rung with the real external-write test rather than relaxing it."
    );

    // …and the decision is not lost: the next launch reads it.
    let relaunched =
        IntegrationsSettingsVm::over_dir(rig.dir.path()).expect("a fresh store, as at boot");
    assert!(
        relaunched
            .rows()
            .iter()
            .find(|r| r.provider == "gmail")
            .expect("gmail row")
            .enabled,
        "the external decision must survive to the next launch — invisible now is \
         acceptable, lost is not"
    );
}
