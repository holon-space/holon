//! A previous run's panic record counts as seen only after a frame that drew
//! its toast inside the viewport, so a run that dies before that, or a window
//! that cannot show it, shows it again at the next start.
//!
//! @pbt kind windowed
//! @pbt covers panic-record-seen-after-draw — the window acknowledges the
//! records `install` showed only on the frame after one whose previous-run
//! toast lay inside the viewport (BugFunnel
//! 2026-10-08-arm-consumes-the-previous-panic-record-unshown); the toast fits
//! every window from the 300x200 minimum up, whatever the message's length or
//! glyph width, and the acknowledged records keep their bytes in the history
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone is headless and
//! draws no frame

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui::AnyWindowHandle;
use gpui::AssetSource;
use gpui::TestApp;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::panic_record::DROPPED_FILE;
use holon_frontend::panic_record::KEPT_UNSHOWN;
use holon_frontend::panic_record::PanicRecord;
use holon_frontend::panic_record::SEEN_DIR;
use holon_frontend::panic_record::UNSHOWN_DIR;
use holon_gpui::launch_holon_window_with_engine_and_share;
use holon_gpui::share_ui::DEGRADED_TOAST_STACK;
use holon_gpui::share_ui::PREVIOUS_RUN_TOAST;
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

/// The bytes of every record file in `dir`, in record order.
fn record_bytes(dir: &Path) -> Vec<Vec<u8>> {
    let mut records: Vec<(u64, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("list {}: {e}", dir.display()))
        .map(|entry| {
            let path = entry.expect("an entry").path();
            let n: u64 = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_else(|| panic!("{} has no text name", path.display()))
                .parse()
                .unwrap_or_else(|e| panic!("{} is not <n>.json: {e}", path.display()));
            (n, std::fs::read(&path).expect("read a record"))
        })
        .collect();
    records.sort();
    records.into_iter().map(|(_, bytes)| bytes).collect()
}

fn the_window(app: &mut TestApp) -> AnyWindowHandle {
    let windows = app.update(|cx| cx.windows());
    assert_eq!(windows.len(), 1, "the launch opens one window");
    windows[0]
}

/// Seeds the records of earlier runs into a config dir, and returns where the
/// newest of them panicked.
type Seed = fn(&Path) -> String;

fn one_previous_run(config: &Path) -> String {
    one_run(config, "the previous run")
}

fn one_run(config: &Path, message: &str) -> String {
    PanicRecord {
        message: message.to_string(),
        location: "prior.rs:1:1".to_string(),
        thread: "main".to_string(),
    }
    .write_to(config)
    .expect("seed the previous run's record");
    "prior.rs:1:1".to_string()
}

/// A panic message of wide glyphs, which a width estimate measured on latin
/// text under-counts.
fn a_wide_glyph_message(config: &Path) -> String {
    one_run(config, &"数据库在启动时崩溃了".repeat(130))
}

/// Ten kept records, each from another site, and the summary of three dropped
/// ones: the most a crash loop leaves `install` to show.
fn a_crash_loop(config: &Path) -> String {
    crash_loop_with(config, |run| {
        format!("called `Result::unwrap()` on an `Err` value: crash loop run {run}")
    })
}

/// [`a_crash_loop`] whose messages are as long as a real `io::Error` unwrap.
fn a_crash_loop_with_long_messages(config: &Path) -> String {
    crash_loop_with(config, |run| {
        let message = format!(
            "called `Result::unwrap()` on an `Err` value: run {run} Os {{ code: 2, kind: \
             NotFound, message: \"No such file or directory\" }} while opening {}",
            "/data/user/0/space.holon.app/files/vault/Projects/Holon/".repeat(10)
        );
        message.chars().take(576).collect()
    })
}

fn crash_loop_with(config: &Path, message: fn(usize) -> String) -> String {
    let unshown_dir = config.join(UNSHOWN_DIR);
    std::fs::create_dir_all(&unshown_dir).expect("create the unshown dir");
    let mut newest = String::new();
    for run in 1..=KEPT_UNSHOWN {
        let record = PanicRecord {
            message: message(run),
            location: format!("crates/holon/src/boot.rs:{}:9", 400 + run),
            thread: "main".to_string(),
        };
        std::fs::write(
            unshown_dir.join(format!("{run}.json")),
            serde_json::to_vec_pretty(&record).expect("serialize a record"),
        )
        .expect("seed a kept record");
        newest = record.location;
    }
    std::fs::write(
        config.join(DROPPED_FILE),
        r#"{"count":3,"first_ended":"2026-10-01T08:00:00Z","last_ended":"2026-10-01T08:02:00Z"}"#,
    )
    .expect("seed the dropped summary");
    newest
}

fn inside(rect: &ElementInfo, width: f32, height: f32) -> bool {
    rect.width > 0.0
        && rect.height > 0.0
        && rect.x >= 0.0
        && rect.y >= 0.0
        && rect.x + rect.width <= width
        && rect.y + rect.height <= height
}

/// Open a window of `size` over a bus `install` filled from `seed`, and check
/// that the frame paints the previous-run toast inside the viewport and that
/// its records move to the history, byte for byte, exactly on the frame after
/// it.
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
    let newest_site = seed(config.path());
    let bus = holon_frontend::panic_record::install(config.path());
    let seeded = unshown(config.path());
    assert!(!seeded.is_empty(), "install shows the records on its bus");
    let seeded_bytes = record_bytes(&config.path().join(UNSHOWN_DIR));

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
    let (width, height) = size.split_once('x').expect("a WxH size");
    let viewport = app.update(|cx| {
        window
            .update(cx, |_, window, _| {
                let size = window.viewport_size();
                (f32::from(size.width), f32::from(size.height))
            })
            .expect("read the viewport")
    });
    assert_eq!(
        (viewport.0.to_string(), viewport.1.to_string()),
        (width.to_string(), height.to_string()),
        "the window opens at {size}"
    );
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
    let lines: Vec<(String, ElementInfo)> = bounds
        .all_elements()
        .into_iter()
        .filter(|(id, _)| id.starts_with(TOAST_LINE))
        .collect();
    let newest_line = lines.iter().find(|(_, info)| {
        info.displayed_text
            .as_deref()
            .is_some_and(|text| text.contains(newest_site.as_str()))
    });
    assert!(
        newest_line.is_some_and(|(_, info)| inside(info, viewport.0, viewport.1)),
        "the frame must paint, inside the {size} viewport, a toast line naming the newest \
         crash site {newest_site}; the toast lines are {:#?}",
        lines
            .iter()
            .map(|(id, info)| (
                id,
                &info.displayed_text,
                (info.x, info.y, info.width, info.height)
            ))
            .collect::<Vec<_>>()
    );
    let stack = bounds
        .element_info(DEGRADED_TOAST_STACK)
        .expect("the toast stack is drawn");
    assert!(
        stack.y > 0.0 && stack.y + stack.height < viewport.1,
        "the toast stack is clipped by the {size} viewport: {stack:?}"
    );
    let toast = bounds
        .element_info(PREVIOUS_RUN_TOAST)
        .expect("the previous-run toast records its box");
    assert!(
        inside(&toast, viewport.0, viewport.1),
        "the previous-run toast lies inside the {size} viewport: {toast:?}"
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
    assert_eq!(
        record_bytes(&config.path().join(SEEN_DIR)),
        seeded_bytes,
        "every acknowledged record keeps its bytes in the history"
    );

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

#[test]
fn a_crash_loop_fits_a_600px_window() {
    records_are_seen_after_a_drawn_frame("1400x600", a_crash_loop);
}

#[test]
fn a_crash_loop_of_long_messages_fits_a_900px_window() {
    records_are_seen_after_a_drawn_frame("1400x900", a_crash_loop_with_long_messages);
}

#[test]
fn a_wide_glyph_message_fits_a_900px_window() {
    records_are_seen_after_a_drawn_frame("1400x900", a_wide_glyph_message);
}

#[test]
fn a_crash_loop_fits_the_smallest_window() {
    records_are_seen_after_a_drawn_frame("300x200", a_crash_loop);
}

mod test_init;
