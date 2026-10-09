//! A builder that panics on every frame reports its panic in full on stderr
//! once, and each repeat of the same panic as one line counting it. Observed
//! from outside the process, whose stderr the test reads.

use std::path::Path;
use std::process::Command;

use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::panic_record;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::render_interpreter::BuilderArgs;
use holon_frontend::shadow_builders::build_shadow_interpreter;

const CHILD_ENV: &str = "HOLON_REPEATED_PANIC_CHILD_DIR";
const MESSAGE: &str = "injected panic on every frame";
const FRAMES: usize = 5;

fn panicking(_: BuilderArgs<'_, ReactiveViewModel>) -> ReactiveViewModel {
    panic!("{MESSAGE}")
}

/// In the child: build a panicking widget for `FRAMES` frames on a bus.
fn build_frames(config_dir: &Path) {
    let _bus = panic_record::install(config_dir);
    let mut interpreter = build_shadow_interpreter();
    interpreter.register("panicking", panicking);
    let services = StubBuilderServices::new().with_interpreter(interpreter);
    let expr = RenderExpr::FunctionCall {
        name: "panicking".to_string(),
        args: vec![],
    };
    for _ in 0..FRAMES {
        services.interpret(&expr, &RenderContext::default());
    }
}

#[test]
fn a_panic_repeated_every_frame_is_reported_in_full_once() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build_frames(Path::new(&dir));
        return;
    }

    let dir = tempfile::tempdir().expect("temp config dir");
    let output = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "--exact",
            "a_panic_repeated_every_frame_is_reported_in_full_once",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, dir.path())
        .output()
        .expect("spawn the child test process");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the child survives its caught panics; stderr:\n{stderr}"
    );

    let full_reports = stderr.lines().filter(|line| *line == MESSAGE).count();
    assert_eq!(
        full_reports, 1,
        "the default hook reports the panic in full exactly once; stderr:\n{stderr}"
    );
    let repeats: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("again") && line.contains(file!()))
        .collect();
    assert_eq!(
        repeats.len(),
        FRAMES - 1,
        "each repeat is one line naming where it panicked; stderr:\n{stderr}"
    );
    assert!(
        repeats
            .last()
            .is_some_and(|line| line.contains(&format!("{FRAMES} times"))),
        "the last repeat counts every occurrence: {repeats:?}"
    );
}
