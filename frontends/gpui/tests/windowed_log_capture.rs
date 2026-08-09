//! The windowed suite's observability floor.
//!
//! Proves the two things a windowed test may assume about logs, and enforces
//! the wiring that makes them true:
//!
//! 1. a `warn!` emitted while a widget RENDERS is capturable (disclosed
//!    degradation is observable, not fatal);
//! 2. an `error!` emitted the same way lands in the problem window — the same
//!    set `inv-no-observed-errors` reds on in the keystone;
//! 3. every `frontends/gpui/tests/*.rs` target declares `mod test_init;`, so a
//!    new windowed target cannot silently reintroduce the discard.

mod test_init;

use gpui::AppContext;
use gpui::Bounds;
use gpui::Context;
use gpui::IntoElement;
use gpui::Point;
use gpui::Render;
use gpui::TestAppContext;
use gpui::Window;
use gpui::WindowBounds;
use gpui::WindowHandle;
use gpui::WindowOptions;
use gpui::div;
use gpui::px;
use gpui::size;

/// Distinctive enough that a match cannot come from unrelated log traffic.
const WARN_PROBE: &str = "windowed-log-capture-warn-probe";
const ERROR_PROBE: &str = "windowed-log-capture-error-probe";

/// A view whose only job is to emit one log event per render pass.
struct LoggingView {
    level: tracing::Level,
}

impl Render for LoggingView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.level == tracing::Level::WARN {
            tracing::warn!(probe = WARN_PROBE, "degraded render disclosure");
        } else {
            tracing::error!(probe = ERROR_PROBE, "render failure disclosure");
        }
        div()
    }
}

fn render_logging_view(cx: &mut TestAppContext, level: tracing::Level) {
    let _window: WindowHandle<LoggingView> = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(800.0), px(600.0)),
                })),
                ..Default::default()
            },
            |_window, cx| cx.new(|_cx| LoggingView { level }),
        )
        .expect("open_window failed")
    });
    cx.run_until_parked();
}

/// RED before the subscriber wiring: the windowed binary installed no
/// subscriber, so this WARN reached no layer and the window stayed empty.
#[gpui::test]
fn warn_emitted_during_render_is_capturable(cx: &mut TestAppContext) {
    test_init::begin_case();

    render_logging_view(cx, tracing::Level::WARN);

    let warnings = test_init::captured_warnings();
    assert!(
        warnings.iter().any(|w| w.message.contains(WARN_PROBE)),
        "a WARN emitted during render must be capturable in a windowed test; captured \
         warnings: {warnings:?}"
    );
    let problems = test_init::captured_problems();
    assert!(
        !problems.iter().any(|p| p.message.contains(WARN_PROBE)),
        "a WARN is a disclosed degradation, not a problem — it must NOT enter the window \
         `inv-no-observed-errors` reds on; problems: {problems:?}"
    );
}

/// The other half of the policy: ERROR is fatal-tier and lands where the
/// keystone's observability invariant looks.
#[gpui::test]
fn error_emitted_during_render_lands_in_the_problem_window(cx: &mut TestAppContext) {
    test_init::begin_case();

    render_logging_view(cx, tracing::Level::ERROR);

    let problems = test_init::captured_problems();
    assert!(
        problems.iter().any(|p| p.message.contains(ERROR_PROBE)),
        "an ERROR emitted during render must land in the problem window; problems: {problems:?}"
    );
}

/// Every root cargo autodiscovers as an integration-test target under
/// `tests/`: a top-level `tests/<name>.rs` AND a `tests/<dir>/main.rs`. The
/// second form is the one a "just scan `*.rs`" check misses — `test_init/`,
/// `support/` and `pbt_harness/` are plain modules with no `main.rs` and are
/// correctly excluded, but a future `tests/newthing/main.rs` would build, run,
/// and install no subscriber.
fn windowed_target_sources(tests_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut targets = Vec::new();
    for entry in std::fs::read_dir(tests_dir).expect("windowed tests dir is readable") {
        let path = entry.expect("dir entry readable").path();
        if path.is_dir() {
            let main = path.join("main.rs");
            if main.is_file() {
                targets.push(main);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            targets.push(path);
        }
    }
    targets.sort();
    targets
}

/// A LIVE declaration, not a mention. `source.contains("mod test_init;")` is
/// satisfied by a commented-out line, which would leave the target silently
/// unsubscribed while the guard stays green.
fn declares_test_init(source: &str) -> bool {
    source
        .lines()
        .any(|line| line.trim_start() == "mod test_init;")
}

/// A windowed target that does not declare `mod test_init;` runs with the
/// no-op global dispatcher and discards every log it emits. Enforced here
/// rather than by review, because the failure mode is silence.
#[test]
fn every_windowed_target_declares_test_init() {
    let tests_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let targets = windowed_target_sources(&tests_dir);
    assert!(
        targets.len() > 40,
        "the sweep found only {} windowed targets — it is scanning the wrong place",
        targets.len()
    );

    let mut missing = Vec::new();
    for path in targets {
        let source = std::fs::read_to_string(&path).expect("test source is readable");
        if !declares_test_init(&source) {
            missing.push(
                path.strip_prefix(&tests_dir)
                    .expect("target lives under tests/")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "these windowed test targets install no tracing subscriber — add `mod test_init;` to \
         each (see frontends/gpui/tests/test_init/mod.rs): {missing:?}"
    );
}

/// The guard above is only worth its line count if it rejects the two shapes
/// that would otherwise slip past it. Pinned here so it cannot rot back into a
/// substring scan over `*.rs`.
#[test]
fn the_sweep_rejects_commented_declarations_and_sees_subdirectory_targets() {
    assert!(declares_test_init("mod test_init;\n"));
    assert!(declares_test_init(
        "use x;\n\n    mod test_init;\nfn main() {}\n"
    ));
    assert!(!declares_test_init("// mod test_init;\n"));
    assert!(!declares_test_init("//mod test_init;\n"));
    assert!(!declares_test_init("/* mod test_init; */\n"));
    assert!(!declares_test_init("// see also: mod test_init;\n"));

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("alpha.rs"), "mod test_init;\n").expect("write alpha");
    std::fs::write(root.join("notes.txt"), "ignored").expect("write notes");
    std::fs::create_dir(root.join("subdir_target")).expect("mkdir target");
    std::fs::write(root.join("subdir_target/main.rs"), "fn main() {}\n").expect("write main");
    std::fs::create_dir(root.join("plain_module")).expect("mkdir module");
    std::fs::write(root.join("plain_module/mod.rs"), "pub fn helper() {}\n").expect("write mod");

    let found: Vec<String> = windowed_target_sources(root)
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        found,
        vec!["alpha.rs".to_string(), "subdir_target/main.rs".to_string()],
        "the sweep must see `<dir>/main.rs` targets and ignore plain module directories"
    );
}
