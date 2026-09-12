//! A refusal the user can act on: none silently dropped, none cut in half.
//!
//! Both halves were found by the `dogfood-explorer` gate for
//! `user-connections`:
//!
//! - Seven refusals produced five toasts. The other two were in the log and
//!   nowhere on screen, with no overflow affordance, so a user cleaning up
//!   several bad files is told about only some of them — and one of the two
//!   that vanished was the foreign-secret-namespace refusal, which is the
//!   security rule. (`docs/Testing/bugfunnel/entries/
//!   2026-09-12-refusal-toasts-push-each-other-off-screen-so-some-refusals-are-never-seen.
//!   md`)
//! - The "not switched on" toast's whole payload is a shell command the user is
//!   meant to run, and it was cut mid-path with an ellipsis. A remedy the user
//!   can only partly see is not a remedy. (`docs/Testing/bugfunnel/entries/
//!   2026-09-12-the-not-switched-on-toast-truncates-the-command-it-tells-the-user-to-run.
//!   md`)
//!
//! `share_ui`'s own tests judge the STRINGS; only a real window judges how many
//! of them reach a screen and whether they fit on it.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! refusal_toasts_reach_the_user_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::share_ui::TOAST_LINE;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_loro::ShareDegraded;
use holon_loro::ShareDegradedReason;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// Both heights the stack must survive: the default, and a short window where
/// far fewer refusals fit and the count line carries more of the load.
const WINDOWS: &[(&str, f32, f32)] = &[("1512x900", 1512.0, 900.0), ("1512x720", 1512.0, 720.0)];

/// One unwrapped line of toast text. A line painted shorter than this has been
/// clipped, whatever its `y` says.
const FULL_LINE_H: f32 = 15.0;

/// The stack's own inset from the window edge (`share_ui.rs`). A line clipped
/// at the border reports its edge AT the border, so a bound of exactly the
/// window size would accept the cut.
const STACK_INSET: f32 = 16.0;

/// More refusals than the stack has ever rendered at once, which is the whole
/// point: the dogfood pass raised seven and saw five.
const CONNECTIONS: &[&str] = &[
    "fixturebox",
    "inlinesecret",
    "oldshape",
    "futureshape",
    "foreignsecret",
    "linkedthing",
    "cleartexthost",
];

/// The real sandbox path from the dogfood pass. The DISCLOSURE it produces is
/// 601 characters — well past `share_ui`'s 320-character detail cap — which is
/// what truncated the remedy on screen.
///
/// The precondition below asserts that, because the first fix here shortened
/// the message until it happened to fit, and a test that cannot tell a real
/// exemption from a shorter sentence proves nothing.
const DIR: &str = "/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/\
                   bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/dogfood-uc/state1/config/\
                   a-vault-checked-out-somewhere-deep-because-people-organise-their-disks/\
                   their-own-way-and-a-disclosure-does-not-get-to-choose-how-long-a-path/\
                   is-before-it-decides-whether-the-user-may-read-the-whole-of-it-or-not/\
                   one-more-level-so-this-is-past-six-hundred-characters-on-its-own-here/\
                   another-segment-here-to-carry-it-comfortably-over-the-six-oh-one-mark/\
                   and-a-final-segment-so-the-state-path-itself-clears-six-hundred-chars/\
                   integrations";

/// The cap `share_ui::toast_message` applies to a detail SENTENCE. Mirrored
/// here so the fixture above stays honestly longer than it.
const MAX_DETAIL_CHARS: usize = 320;

fn remedy_for(connection: &str) -> String {
    format!("HOLON_MCP_INTEGRATIONS_DIR='{DIR}' scripts/holon-integration-enable.sh {connection}")
}

fn state_path_of(connection: &str) -> String {
    format!("{DIR}/{connection}.state.toml")
}

#[test]
fn every_refusal_is_either_shown_in_full_or_counted() {
    for (window, w, h) in WINDOWS {
        run_at(window, *w, *h);
    }
}

/// One window's worth of the property. A SHORT window is not a smaller version
/// of a tall one: it admits fewer toasts, so it is where a stack that sizes
/// itself wrongly clips first.
fn run_at(window: &str, window_w: f32, window_h: f32) {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", window) };
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));

    let set = ComponentSet::full_headless();
    let bundle = runtime
        .block_on(async { compose_sut_windowed_base_seeded(&set, &resolver, &[], &[]).await });
    let session = bundle
        .session
        .clone()
        .expect("full_headless -> booted FrontendSession");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> booted ReactiveEngine");
    let frontend = bundle
        .frontend
        .clone()
        .expect("full_headless -> booted frontend component");
    let bus = frontend
        .degraded_bus()
        .expect("the booted session's degraded bus");

    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                None,
                Some(bus.clone()),
                "Holon-RefusalToasts-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // One refusal per connection, over the session's own bus, so the toasts are
    // built by the production bridge.
    for connection in CONNECTIONS {
        bus.emit(ShareDegraded {
            shared_tree_id: (*connection).into(),
            reason: ShareDegradedReason::IntegrationNotEnabled {
                integration: (*connection).to_string(),
                installed_path: format!("{DIR}/{connection}.yaml"),
                state_path: state_path_of(connection),
                remedy: remedy_for(connection),
            },
        });
    }

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let lines: Vec<(String, ElementInfo)> = bounds
        .all_elements()
        .into_iter()
        .filter(|(id, _)| id.starts_with(TOAST_LINE))
        .collect();

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    let painted: Vec<String> = lines
        .iter()
        .filter_map(|(_, i)| i.displayed_text.as_deref().map(str::to_string))
        .collect();
    assert!(
        !painted.is_empty(),
        "precondition: no toast line painted at all, so the assertions below would judge a stack \
         that never opened"
    );
    let widest = CONNECTIONS
        .iter()
        .map(|c| remedy_for(c).chars().count() + state_path_of(c).chars().count())
        .max()
        .expect("CONNECTIONS is not empty");
    assert!(
        widest > MAX_DETAIL_CHARS,
        "precondition: the remedy plus the state path ({widest} chars) must exceed the \
         {MAX_DETAIL_CHARS}-char cap, else this test cannot tell a real exemption from a message \
         that merely got shorter"
    );

    // The whole painted stack, printed on every run: a geometry failure below is
    // unreadable without the neighbours that pushed the offender off-screen.
    for (id, i) in &lines {
        println!(
            "  {id:<28} x={:>7.1} y={:>7.1} w={:>7.1} h={:>6.1}  {:?}",
            i.x,
            i.y,
            i.width,
            i.height,
            i.displayed_text.as_deref().unwrap_or("")
        );
    }

    // 1. Nothing is lost. A connection is accounted for when it is named on screen;
    //    the ones that are not must be COUNTED on screen, because a user who cannot
    //    see a refusal cannot go and fix the file it names.
    let shown: Vec<&str> = CONNECTIONS
        .iter()
        .copied()
        .filter(|c| painted.iter().any(|l| l.contains(c)))
        .collect();
    let hidden = CONNECTIONS.len() - shown.len();
    // The count lives in its own tracked element, so reading THAT element's
    // text is not the vacuous "some painted line contains a digit" — these
    // messages are full of digits. A missing element is the real failure: it is
    // what left two refusals unmentioned in the dogfood pass.
    let overflow_line = lines.iter().find(|(id, _)| id.contains("overflow"));
    if hidden > 0 {
        let (_, info) = overflow_line.unwrap_or_else(|| {
            panic!(
                "{hidden} of {} refusals are not named on screen and NO count line was painted, \
                 so the user is never told they exist. Shown: {shown:?}. Painted: {painted:#?}",
                CONNECTIONS.len()
            )
        });
        let text = info.displayed_text.as_deref().unwrap_or("");
        assert!(
            text.contains(&hidden.to_string()),
            "the count line does not carry the number hidden ({hidden}): {text:?}"
        );
    } else {
        assert!(
            overflow_line.is_none(),
            "nothing is hidden, so a count line would be a lie: {:?}",
            overflow_line.map(|(_, i)| i.displayed_text.clone())
        );
    }

    // 2. What is shown is shown whole. An ellipsis in a disclosure means the clause
    //    that was cut — the file to edit, the tail of a command — is reachable
    //    nowhere on screen, and toast text cannot be selected.
    for line in &painted {
        assert!(
            !line.contains('…'),
            "a refusal is painted with its content cut off: what follows the ellipsis is the file \
             the user has to go and fix, and toast text cannot be selected or copied. Line: {line}"
        );
    }

    // 2b. Both payloads arrive WHOLE: the command to run (prior ruling D2 — a
    //     remedy cut in half reads as complete and does not work) and the file
    //     it writes (D1). Together they are longer than the cap, so a toast
    //     that still put them in the capped sentence would fail here.
    for connection in &shown {
        for payload in [remedy_for(connection), state_path_of(connection)] {
            assert!(
                painted.iter().any(|l| l.contains(&payload)),
                "the toast for '{connection}' does not paint this whole:\n  {payload}\nPainted: \
                 {painted:#?}"
            );
        }
    }

    // 3. A refusal that IS shown says where to act on it. The toast is a
    //    notification, not a terminal — the enable command lives in the log, which
    //    is readable and copyable; what the toast owes the user is the connection's
    //    name and the surface that fixes it.
    for connection in &shown {
        assert!(
            painted
                .iter()
                .any(|l| l.contains(connection) && l.contains("Settings")),
            "the toast for '{connection}' is on screen but does not say where to switch it on, so \
             it reports a problem with no way to act on it. Painted: {painted:#?}"
        );
    }

    // 3b. The COUNT is visible. It is the one line that must never be the
    //     clipped one: a stack that loses it is indistinguishable from a stack
    //     with nothing hidden, and the user is back to never learning that
    //     refusals exist. Checked as GEOMETRY, not just as text, because being
    //     in the painted list is what the clipped lines below also were.
    if let Some((_, info)) = overflow_line {
        // A line clipped at the TOP reports y=0 with a sliver of height, which
        // an "h > 0 and bottom inside the window" check waves through — that is
        // exactly how a 2px-tall count line passed an earlier version of this
        // assertion. So: a FULL line, and clear of the top inset.
        assert!(
            info.height >= FULL_LINE_H
                && info.y >= STACK_INSET
                && info.y + info.height <= window_h - STACK_INSET,
            "the count line is clipped (y={} h={}) in the {window} window, so the user is never \
             told that {hidden} refusals exist. It is painted FIRST precisely so it cannot be the \
             one that gets cut.",
            info.y,
            info.height
        );
    }

    // 4. What is painted is inside the window, WITH a real height. A line at h=0 is
    //    in the element tree and on nobody's screen — the failure mode a text-only
    //    assertion cannot see. A line that runs off the edge is the disclosure not
    //    arriving, the same failure by a different route.
    for (id, info) in &lines {
        assert!(
            info.width > 0.0
                && info.height > 0.0
                && info.x >= 0.0
                && info.y >= STACK_INSET
                && info.x + info.width <= window_w - STACK_INSET
                && info.y + info.height <= window_h - STACK_INSET,
            "the toast line {id} must lie inside the {window} window. It sits at x={} y={} w={} \
             h={} and reads {:?}",
            info.x,
            info.y,
            info.width,
            info.height,
            info.displayed_text
        );
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
