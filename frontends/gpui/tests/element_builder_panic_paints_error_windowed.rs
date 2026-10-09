//! A GPUI element builder that panics while a window renders paints as an
//! error naming the builder, the panic message, its location and thread,
//! between siblings that paint as usual; the panic shows on the condition bus
//! once and is not reported again as a crash at the next start.
//!
//! The panic is a real one: a `view_mode_switcher` node without a slot trips
//! the builder's `expect`.
//!
//! Run: `cargo test -p holon-gpui --test
//! element_builder_panic_paints_error_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe). Own test binary: the panic hook is process-global.

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::Value;
use holon_frontend::panic_record;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use support::render_reactive_fixture_quiescent_sized;

const MESSAGE: &str = "view_mode_switcher requires a slot";
const BUILDER_FILE: &str = "view_mode_switcher.rs";

fn shown_panics(bus: &ConditionBus) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let shown: Vec<String> = bus
            .current()
            .into_iter()
            .filter(|c| matches!(&c.reason, ConditionKind::TaskPanicked { message, .. } if message == MESSAGE))
            .map(|c| c.subject)
            .collect();
        if !shown.is_empty() || Instant::now() >= deadline {
            return shown;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
fn a_panicking_element_builder_paints_an_error_between_its_siblings(cx: &mut TestAppContext) {
    let config = tempfile::tempdir().expect("a config dir");
    let bus = panic_record::install(config.path());

    let tree = ReactiveViewModel::from_widget(
        "column",
        HashMap::from([("gap".to_string(), Value::Float(4.0))]),
    )
    .with_children(vec![
        ReactiveViewModel::text("before"),
        ReactiveViewModel::from_widget("view_mode_switcher", HashMap::new()),
        ReactiveViewModel::text("after"),
    ]);

    let snap =
        render_reactive_fixture_quiescent_sized(cx, Arc::new(tree), size(px(900.0), px(600.0)));

    let painted_errors: Vec<String> = snap
        .of_type("error_message")
        .filter_map(|info| info.displayed_text.as_deref().map(str::to_string))
        .collect();
    let [error] = painted_errors.as_slice() else {
        panic!(
            "expected exactly one painted error, got {painted_errors:?}:\n{}",
            snap.dump()
        );
    };
    let thread = std::thread::current();
    let thread = thread.name().expect("a test runs on a named thread");
    for part in ["view_mode_switcher", MESSAGE, BUILDER_FILE, thread] {
        assert!(
            error.contains(part),
            "the painted error must name {part:?}: {error:?}"
        );
    }
    assert_eq!(
        snap.of_type("text").count(),
        2,
        "both siblings of the panicking element must paint:\n{}",
        snap.dump()
    );

    let shown = shown_panics(&bus);
    let [subject] = shown.as_slice() else {
        panic!("the caught panic must show on the condition bus exactly once, got {shown:?}");
    };
    assert!(
        subject.contains(BUILDER_FILE),
        "the condition names where the builder panicked: {subject:?}"
    );
    assert!(
        !config.path().join(panic_record::RECORD_FILE).exists(),
        "a caught and shown panic must not be disclosed again as a crash at the next start"
    );
}

mod test_init;
