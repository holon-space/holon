//! The windowed suite's observability floor.
//!
//! Proves the two things a windowed test may assume about logs, and enforces
//! the wiring that makes them true:
//!
//! 1. a `warn!` emitted while a widget RENDERS is capturable (disclosed
//!    degradation is observable, not fatal);
//! 2. an `error!` emitted the same way lands in the problem window — the same
//!    set `inv-no-observed-errors` reds on in the keystone;
//! 3. every `[[test]]` target of the package declares `mod test_init;`, so a
//!    new windowed target cannot silently reintroduce the discard, and every
//!    `tests/*.rs` file is compiled into one of them.

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

use crate::test_init;

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

/// Every integration-test target of the package: its `[[test]]` entries.
/// `autotests = false`, so no other file under `tests/` builds as a target.
fn windowed_target_sources(manifest_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let manifest: toml::Table = toml::from_str(
        &std::fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("Cargo.toml is readable"),
    )
    .expect("Cargo.toml parses");
    let mut targets: Vec<_> = manifest["test"]
        .as_array()
        .expect("the package lists its `[[test]]` targets")
        .iter()
        .map(|target| {
            let path = match target.get("path") {
                Some(path) => path
                    .as_str()
                    .expect("a `[[test]]` path is a string")
                    .to_owned(),
                None => format!(
                    "tests/{}.rs",
                    target["name"]
                        .as_str()
                        .expect("a `[[test]]` name is a string")
                ),
            };
            manifest_dir.join(path)
        })
        .collect();
    targets.sort();
    targets
}

/// The `tests/*.rs` files that are neither a target nor a module of
/// `tests/main.rs`: cargo compiles them nowhere, so their tests never run.
fn unreached_test_files(manifest_dir: &std::path::Path) -> Vec<String> {
    let tests_dir = manifest_dir.join("tests");
    let targets = windowed_target_sources(manifest_dir);
    let main =
        std::fs::read_to_string(tests_dir.join("main.rs")).expect("tests/main.rs is readable");
    let modules: Vec<&str> = main
        .lines()
        .filter_map(|line| line.trim().strip_prefix("mod ")?.strip_suffix(';'))
        .collect();
    let mut unreached = Vec::new();
    for entry in std::fs::read_dir(&tests_dir).expect("windowed tests dir is readable") {
        let path = entry.expect("dir entry readable").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") || targets.contains(&path) {
            continue;
        }
        let stem = path
            .file_stem()
            .expect("a .rs file has a stem")
            .to_string_lossy();
        if !modules.contains(&stem.as_ref()) {
            unreached.push(stem.into_owned());
        }
    }
    unreached.sort();
    unreached
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
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let targets = windowed_target_sources(manifest_dir);
    assert!(
        targets.contains(&manifest_dir.join("tests/main.rs")),
        "the sweep did not find tests/main.rs among {targets:?} — it is reading the wrong manifest"
    );

    let mut missing = Vec::new();
    for path in targets {
        let source = std::fs::read_to_string(&path).expect("test source is readable");
        if !declares_test_init(&source) {
            missing.push(
                path.strip_prefix(manifest_dir)
                    .expect("target lives under the package")
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

/// A new `tests/<name>.rs` builds only once `tests/main.rs` declares it (or
/// Cargo.toml lists it as a target); until then it is silently dead.
#[test]
fn every_windowed_test_file_is_compiled() {
    let unreached = unreached_test_files(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    assert!(
        unreached.is_empty(),
        "these tests/*.rs files are compiled into no target — add `mod <name>;` to \
         tests/main.rs: {unreached:?}"
    );
}

/// The guards above are only worth their line count if they reject the shapes
/// that would otherwise slip past them. Pinned here so they cannot rot back
/// into a substring scan.
#[test]
fn the_sweep_rejects_commented_declarations_and_reads_the_manifest() {
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
    std::fs::write(
        root.join("Cargo.toml"),
        "[[test]]\nname = \"main\"\npath = \"tests/main.rs\"\n\n[[test]]\nname = \"alone\"\nharness = false\n",
    )
    .expect("write manifest");
    std::fs::create_dir(root.join("tests")).expect("mkdir tests");
    std::fs::write(
        root.join("tests/main.rs"),
        "mod test_init;\nmod declared;\n",
    )
    .expect("write main");
    std::fs::write(root.join("tests/alone.rs"), "fn main() {}\n").expect("write alone");
    std::fs::write(root.join("tests/declared.rs"), "").expect("write declared");
    std::fs::write(root.join("tests/forgotten.rs"), "").expect("write forgotten");
    std::fs::write(root.join("tests/notes.txt"), "ignored").expect("write notes");

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
        vec!["tests/alone.rs".to_string(), "tests/main.rs".to_string()],
        "the sweep must read `[[test]]` paths, defaulting to tests/<name>.rs"
    );
    assert_eq!(
        unreached_test_files(root),
        vec!["forgotten".to_string()],
        "a file that is neither a target nor declared in main.rs must be reported"
    );
}
