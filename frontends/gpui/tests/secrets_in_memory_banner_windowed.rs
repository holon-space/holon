//! A session that saves no credential says so ON SCREEN.
//!
//! `HOLON_SECRETS_BACKEND=memory` raises a sticky `SecretsHeldInMemory`
//! condition, and `holon-app/tests/in_memory_secret_backend_boot.rs` asserts it
//! reaches the bus. That is not the same claim as the user seeing it: a
//! condition raised on a bus nobody renders is the silent degradation this mode
//! exists to avoid, and the whole justification for admitting an in-memory
//! credential store is that the user is TOLD their token is not being saved.
//!
//! So this rung asserts the words, painted, in a real window over a real boot —
//! the half a headless bus assertion structurally cannot see.
//!
//! It also seeds ONE fixture secret, which is what the 2026-09-12 re-check was
//! looking at: the banner named the seed file on the same capped prose line as
//! its standing sentence, so the cap fell inside the path and only the
//! `/private/var/folders/hc/…` prefix — the half every macOS temp path shares —
//! ever painted. The banner exists to stop a screenshot passing a fixture off
//! as a real credential, so it owes the reader WHICH fixture. The same sentence
//! read "1 seeded fixture secrets".
//!
//! The bus is handed to the launcher explicitly. `launch_holon_window_*` takes
//! it as an `Option`, and passing `None` (which the other windowed rungs in
//! this lane do, having nothing to disclose) renders no banner at all — so a
//! test that forgot it would pass on an empty window.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! secrets_in_memory_banner_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe), and this binary sets a process-global environment variable.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::sync::Arc;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::ConditionBus;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::test_environment::TestEnvironment;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The sentence the banner must carry. Taken from the fact that changes what a
/// user does — that the credential does not survive the session — rather than
/// from the mode's name, which tells them nothing.
const MUST_SAY: &str = "gone when Holon exits";

/// The seed file's own name. Distinctive, and deliberately at the TAIL of a
/// long temp path: the 2026-09-12 re-check saw the banner cut inside
/// `/private/var/folders/hc/…`, which is the half every macOS temp path shares,
/// so the half that says WHICH fixture planted the credentials was the half
/// that never painted
/// (`docs/Testing/bugfunnel/entries/
/// 2026-09-12-the-seeded-secrets-banner-cuts-its-own-file-path-and-miscounts-in-words.md`).
const SEED_FILE: &str = "windowed-seed-fixture.toml";

/// ONE entry, because the count's singular arm is the second half of the same
/// finding: the banner read "1 seeded fixture secrets".
const SEED_KEY: &str = "windowed.fixture.token";
const SEED_VALUE: &str = "synthetic-not-a-real-credential";

/// What a correctly-numbered banner says about one planted secret. The plural
/// form is what shipped.
const SINGULAR: &str = "1 seeded fixture secret was";
const MISCOUNT: &str = "1 seeded fixture secrets";

fn painted_text(bounds: &BoundsRegistry) -> Vec<String> {
    let mut out: Vec<String> = bounds
        .all_elements()
        .into_iter()
        .filter_map(|(_, info)| info.displayed_text)
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn an_in_memory_secret_session_paints_the_banner_that_admits_it() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let home = tempfile::tempdir().expect("tempdir for HOME");

    // The seed lives DEEP, under the machine's own temp root, because the
    // finding is about a path long enough for a cap to land inside it. A short
    // fixture path would fit whole and the case would prove nothing.
    let seed_dir = tempfile::tempdir().expect("tempdir for the seed fixture");
    let seed_path = seed_dir
        .path()
        .join("a-directory-nested-deeply-enough-that-the-whole-path-is-long")
        .join("and-another-segment-so-a-three-hundred-and-twenty-char-cap-bites")
        .join(SEED_FILE);
    std::fs::create_dir_all(seed_path.parent().expect("the seed path has a parent"))
        .expect("create the seed fixture's directory");
    std::fs::write(&seed_path, format!("\"{SEED_KEY}\" = \"{SEED_VALUE}\"\n"))
        .expect("write the one-entry seed fixture");

    // SAFETY: single-threaded test binary (`--test-threads=1`), all set before
    // the app boots and before any thread reads the environment. The backend
    // variable is what this rung is about; `TestEnvironment`'s config dir is a
    // `TempDir`, so the admission rule accepts it.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var(holon_secrets::BACKEND_ENV, "memory");
        std::env::set_var(holon_secrets::BACKEND_SEED_ENV, &seed_path);
    }

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime
        .block_on(async { TestEnvironment::new(runtime.clone()) })
        .expect("test environment");
    runtime.block_on(async {
        env.start_app(true)
            .await
            .expect("the app must boot on the in-memory secret backend")
    });

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let bus: Arc<ConditionBus> = (*env
        .injector()
        .expect("injector after start_app")
        .resolve::<Arc<ConditionBus>>())
    .clone();

    // The condition is raised during boot DI, before any window exists. It is
    // STICKY precisely so a window opened afterwards still receives it; if that
    // stopped being true this rung would go red for the right reason.
    assert!(
        bus.subscribe().current.iter().any(|c| matches!(
            c.reason,
            holon_api::ConditionKind::SecretsHeldInMemory { .. }
        )),
        "precondition: the boot raised the condition — without it this rung would be asserting \
         that a window paints a banner nobody sent"
    );

    let bounds = BoundsRegistry::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session,
                engine,
                runtime.handle().clone(),
                NavigationState::new(),
                bounds.clone(),
                None,
                Some(bus),
                "Holon-SecretsInMemoryBanner-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let painted = painted_text(&bounds);

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);

    assert!(
        painted.iter().any(|t| t.contains(MUST_SAY)),
        "a session holding every secret in RAM must SAY so on screen — that disclosure is the \
         whole reason the mode is allowed to exist, and a user who types a token into a field \
         that discards it has been told nothing. Painted text: {painted:#?}"
    );

    // ── The seed disclosure names the fixture ──────────────────────────────
    // Non-vacuity first: a path short enough to survive the cap would make the
    // claim below free.
    let path_str = seed_path.display().to_string();
    assert!(
        path_str.chars().count() > 120,
        "precondition: the seed path ({} chars) must be long enough that a prose cap can land \
         inside it — {path_str}",
        path_str.chars().count()
    );
    assert!(
        painted.iter().any(|t| t.contains(SEED_FILE)),
        "the banner must name WHICH file planted the credentials this session will read as \
         configured. The directory half is shared by every temp path on the machine; the file \
         name is the only distinguishing part, and it sits at the tail — so a path carried on a \
         capped prose line loses exactly the half that matters. Expected {SEED_FILE:?} somewhere \
         in the painted text: {painted:#?}"
    );

    // ── The count agrees with itself ───────────────────────────────────────
    assert!(
        !painted.iter().any(|t| t.contains(MISCOUNT)),
        "the banner says {MISCOUNT:?} about a one-entry fixture. A disclosure that cannot count \
         the thing it is disclosing reads as machine noise rather than as a warning. Painted \
         text: {painted:#?}"
    );
    assert!(
        painted.iter().any(|t| t.contains(SINGULAR)),
        "one planted secret must be announced in the singular ({SINGULAR:?}). Painted text: \
         {painted:#?}"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
