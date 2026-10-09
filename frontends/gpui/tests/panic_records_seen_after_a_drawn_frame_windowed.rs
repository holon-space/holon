//! A previous run's panic record counts as seen only after a frame that drew
//! it, so a run that dies before that shows it again at the next start.
//!
//! @pbt kind windowed
//! @pbt covers panic-record-seen-after-draw — the window acknowledges the
//! records `install` showed only on the frame after the one that painted their
//! toast (BugFunnel 2026-10-08-arm-consumes-the-previous-panic-record-unshown);
//! a crash loop's records are painted in a 900px window and acknowledged with
//! it
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone is headless and
//! draws no frame

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui::AnyWindowHandle;
use gpui::AssetSource;
use gpui::TestApp;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::panic_record::DROPPED_FILE;
use holon_frontend::panic_record::KEPT_UNSHOWN;
use holon_frontend::panic_record::PanicRecord;
use holon_frontend::panic_record::SEEN_RECORD_FILE;
use holon_frontend::panic_record::UNSHOWN_DIR;
use holon_gpui::launch_holon_window_with_engine_and_share;
use holon_gpui::share_ui::TOAST_LINE;
use holon_integration_tests::test_environment::TestEnvironment;

/// The records `install` showed and nothing has marked seen yet.
fn unshown(dir: &Path) -> Vec<String> {
    let mut left: Vec<String> = std::fs::read_dir(dir.join(UNSHOWN_DIR))
        .expect("list the unshown records")
        .map(|entry| {
            entry
                .expect("an unshown entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    if dir.join(DROPPED_FILE).exists() {
        left.push(DROPPED_FILE.to_string());
    }
    left.sort();
    left
}

fn the_window(app: &mut TestApp) -> AnyWindowHandle {
    let windows = app.update(|cx| cx.windows());
    assert_eq!(windows.len(), 1, "the launch opens one window");
    windows[0]
}

/// Seeds the records of earlier runs into a config dir, and returns the panic
/// sites the window must paint.
type Seed = fn(&Path) -> Vec<String>;

fn one_previous_run(config: &Path) -> Vec<String> {
    PanicRecord {
        message: "the previous run".to_string(),
        location: "prior.rs:1:1".to_string(),
        thread: "main".to_string(),
    }
    .write_to(config)
    .expect("seed the previous run's record");
    vec!["prior.rs:1:1".to_string()]
}

/// Ten kept records, each from another site, and the summary of three dropped
/// ones: the most a crash loop leaves `install` to show.
fn a_crash_loop(config: &Path) -> Vec<String> {
    let unshown_dir = config.join(UNSHOWN_DIR);
    std::fs::create_dir_all(&unshown_dir).expect("create the unshown dir");
    let mut sites = Vec::new();
    for run in 1..=KEPT_UNSHOWN {
        let record = PanicRecord {
            message: format!("called `Result::unwrap()` on an `Err` value: crash loop run {run}"),
            location: format!("crates/holon/src/boot.rs:{}:9", 400 + run),
            thread: "main".to_string(),
        };
        std::fs::write(
            unshown_dir.join(format!("{run}.json")),
            serde_json::to_vec_pretty(&record).expect("serialize a record"),
        )
        .expect("seed a kept record");
        sites.push(record.location);
    }
    std::fs::write(
        config.join(DROPPED_FILE),
        r#"{"count":3,"first_ended":"2026-10-01T08:00:00Z","last_ended":"2026-10-01T08:02:00Z"}"#,
    )
    .expect("seed the dropped summary");
    sites
}

/// Open a window of `size` over a bus `install` filled from `seed`, and check
/// that the frame paints where every run panicked and that its records are
/// seen exactly on the frame after it.
fn records_are_seen_after_a_drawn_frame(size: &str, seed: Seed) {
    // Read by `launch_holon_window_impl`; set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", size) };
    let mut app = TestApp::with_text_system_and_assets(
        gpui_platform::current_text_system(),
        Arc::new(()) as Arc<dyn AssetSource>,
    );
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    env.set_enable_loro(false);
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });
    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

    let config = tempfile::tempdir().expect("temp config dir");
    let sites = seed(config.path());
    let bus = holon_frontend::panic_record::install(config.path());
    let seeded = unshown(config.path());
    assert!(!seeded.is_empty(), "install shows the records on its bus");

    let bounds = app.update(|cx| {
        launch_holon_window_with_engine_and_share(
            session,
            engine,
            debug_services,
            None,
            bus.clone(),
            runtime.handle().clone(),
            cx,
        )
    });
    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
    app.run_until_parked();
    let window = the_window(&mut app);
    let (_, height) = size.split_once('x').expect("a WxH size");
    let viewport_h = app.update(|cx| {
        window
            .update(cx, |_, window, _| f32::from(window.viewport_size().height))
            .expect("read the viewport")
    });
    assert_eq!(viewport_h.to_string(), height, "the window opens at {size}");
    assert_eq!(
        unshown(config.path()),
        seeded,
        "the window holds the toast but has drawn no frame with it"
    );

    app.update(|cx| {
        window
            .update(cx, |_, window, cx| window.draw(cx).clear(cx))
            .expect("draw the window");
    });
    bounds.flush();
    let painted: Vec<String> = bounds
        .all_elements()
        .into_iter()
        .filter(|(id, _)| id.starts_with(TOAST_LINE))
        .filter_map(|(_, info)| info.displayed_text.as_deref().map(str::to_string))
        .collect();
    let unpainted: Vec<&String> = sites
        .iter()
        .filter(|site| !painted.iter().any(|line| line.contains(site.as_str())))
        .collect();
    assert!(
        unpainted.is_empty(),
        "the frame must paint where every earlier run panicked; {unpainted:?} are missing \
         from the toast lines {painted:#?}"
    );
    assert_eq!(
        unshown(config.path()),
        seeded,
        "the frame that draws the toast is not yet presented"
    );

    let ran = app.update(|cx| {
        window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .expect("deliver the next frame")
    });
    assert!(
        ran > 0,
        "the drawn frame scheduled the acknowledgement of {seeded:?}"
    );
    assert_eq!(
        unshown(config.path()),
        Vec::<String>::new(),
        "the next frame marks every shown record seen"
    );
    assert!(config.path().join(SEEN_RECORD_FILE).exists());

    std::mem::forget(app);
    std::mem::forget(env);
}

#[test]
fn a_previous_run_is_seen_on_the_frame_after_the_one_that_drew_it() {
    records_are_seen_after_a_drawn_frame("1400x900", one_previous_run);
}

#[test]
fn a_crash_loop_is_painted_and_seen_on_the_frame_after_the_one_that_drew_it() {
    records_are_seen_after_a_drawn_frame("1400x900", a_crash_loop);
}

mod test_init;
