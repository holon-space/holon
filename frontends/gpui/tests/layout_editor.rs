//! Editor-layer fast-UI tests.
//!
//! Exercises `EditorViewModel` end-to-end with a real `LinkProvider`
//! running against canned `popup_query` results via `TestServices`. No
//! GPUI views, no SQL backend — just the controller, popup menu, and
//! provider pipeline, so regressions in the refactored `BuilderServices`
//! plumbing surface here before they ever reach a running app.
//!
//! See the hand-off notes next to the Phase 2 refactor for context: the
//! doc-link autocomplete was the narrowly-missed regression vector when
//! `LinkProvider` switched from `Arc<FrontendSession>` to
//! `Arc<dyn BuilderServices>`, so these tests lock in the round-trip.

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::Value;
use holon_api::render_types::OperationWiring;
use holon_frontend::editor_view_model::EditorAction;
use holon_frontend::editor_view_model::EditorKey;
use holon_frontend::editor_view_model::EditorViewModel;
use holon_frontend::input_trigger::InputTrigger;
use holon_frontend::reactive::BuilderServices;
use support::TestServices;

/// Build a LinkCandidate with the shape LinkProvider expects.
fn row(id: &str, label: &str) -> holon_api::LinkCandidate {
    holon_api::LinkCandidate {
        id: holon_api::EntityUri::parse(id).expect("test fixture id must be a valid EntityUri"),
        label: label.to_string(),
    }
}

fn doc_link_controller() -> EditorViewModel {
    // Minimal operation wiring — LinkProvider doesn't care about operations,
    // but EditorViewModel::new expects a non-empty-ish list when a
    // `set_field` path exists. We don't exercise that path here, so an
    // empty vec is fine.
    let ops: Vec<OperationWiring> = Vec::new();
    let triggers = vec![InputTrigger::TextPrefix {
        prefix: "[[".to_string(),
        action: "doc_link".to_string(),
        at_line_start: false,
        word_boundary: false,
    }];
    let context = HashMap::from([("id".into(), Value::String("block-1".into()))]);
    EditorViewModel::new(ops, triggers, context, "content".into(), String::new())
}

/// Full round-trip: type `[[Proj`, popup activates backed by `LinkProvider`,
/// the signal pipeline emits canned rows through `popup_query`, selecting
/// the first item with `Enter` returns `InsertText` with the resolved
/// `[[id][label]]` link.
///
/// Guards the architectural contract of Phase 2: `LinkProvider` depends on
/// `Arc<dyn BuilderServices>`, not `Arc<FrontendSession>`, and the whole
/// path is testable without a GPUI view, a real tokio-backed query, or a
/// backend. Any regression in the plumbing (services dropped between
/// `EditorViewModel` and `LinkProvider`, popup activation skipping
/// provider construction, `on_select` format drift) fails this test.
#[test]
fn doc_link_round_trip_emits_resolved_insert() {
    let services = TestServices::with_popup_results(vec![
        row("block:proj-alpha", "Project Alpha"),
        row("block:proj-beta", "Project Beta"),
    ]);
    let handle = services.runtime_handle();

    let mut ctrl = doc_link_controller();
    ctrl.set_async_context(services.clone() as Arc<dyn BuilderServices>);

    // Type `[[Proj` → doc_link trigger fires, popup activates, signal is
    // returned. The signal wraps `LinkProvider::candidates` which spawns
    // a `popup_query` future on the runtime from `services`.
    let action = ctrl.on_text_changed("see [[Proj", 10);
    let signal = match action {
        EditorAction::PopupActivated { signal } => signal,
        other => panic!("expected PopupActivated, got {other:?}"),
    };
    assert!(
        ctrl.is_popup_active(),
        "popup should be active after activation"
    );

    // Drive the signal to its first non-empty emission. `map_future` emits
    // `None` while the spawned query is pending and `Some(items)` once it
    // resolves; our `.map(unwrap_or_default)` in `LinkProvider` collapses
    // that to `Vec<PopupItem>`, so the first tick is empty and the second
    // carries the canned rows. Entering the tokio handle lets the spawned
    // task make progress while `futures::executor::block_on` drives the
    // outer stream.
    let items = {
        use futures::StreamExt;
        use futures_signals::signal::SignalExt;
        let _guard = handle.enter();
        futures::executor::block_on(async move {
            let mut stream = Box::pin(signal.to_stream());
            for _ in 0..20 {
                if let Some(items) = stream.next().await {
                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            panic!("signal never produced non-empty items after 20 ticks");
        })
    };

    // The signal closure in `PopupMenu::activate` writes every emission
    // into `popup.items`, so pumping the signal is what lets
    // `on_key(Enter)` find a selected item to forward to
    // `LinkProvider::on_select`.
    assert!(
        items.iter().any(|i| i.id == "block:proj-alpha"),
        "canned row `block:proj-alpha` not in popup items: {items:?}"
    );
    assert!(
        items.iter().any(|i| i.id.starts_with("__create_new__")),
        "LinkProvider should append a 'Create new' entry: {items:?}"
    );

    // Enter selects the first canned row (selected_index starts at 0, which
    // is the first real result since items come from the DB query first and
    // 'Create new' is appended last).
    match ctrl.on_key(EditorKey::Enter) {
        EditorAction::InsertText {
            replacement,
            prefix_start,
        } => {
            assert_eq!(replacement, "[[block:proj-alpha][Project Alpha]]");
            // `prefix_start` is the column where `[[` began in the line;
            // `on_text_changed("see [[Proj", 10)` puts `[[` at column 4.
            assert_eq!(prefix_start, 4);
        }
        other => panic!("expected InsertText, got {other:?}"),
    }

    assert!(
        !ctrl.is_popup_active(),
        "popup should dismiss after a selection"
    );
}

/// Windowed click → caret-placement tests.
///
/// These open a real (headless) gpui window, render a `rendered_text` block,
/// simulate a left click at a known character x-position, and assert the click
/// armed the caret seed at the corresponding buffer offset (via
/// `TestServices::recorded_caret`, the same `set_focus_with_caret` authority
/// the editor mount reads through `peek_caret_seed`).
///
/// Coordinates are deterministic under gpui's `NoopTextSystem`: every BMP glyph
/// advances `em_width = text_sm(14px) * 600/1000 = 8.4px` (astral/emoji glyphs
/// advance double, which these fixtures avoid). The clicked byte offset in the
/// read projection equals the buffer offset today (styled text == stripped
/// content; identity `styled_offset_to_buffer_offset`) — raw-edit I2 swaps that
/// one function for a `RawOffsetMap` lookup without touching this test's shape.
mod windowed_caret {
    use std::collections::HashMap;
    use std::sync::Arc;

    use gpui::MouseButton;
    use gpui::MouseDownEvent;
    use gpui::MouseMoveEvent;
    use gpui::MouseUpEvent;
    use gpui::Pixels;
    use gpui::Point;
    use gpui::TestAppContext;
    use gpui::VisualTestContext;
    use gpui::point;
    use gpui::px;
    use holon_api::EntityRef;
    use holon_api::EntityUri;
    use holon_api::InlineMark;
    use holon_api::MarkSpan;
    use holon_api::Value;
    use holon_api::marks_to_json;
    use holon_api::widget_spec::DataRow;
    use holon_frontend::reactive::BuilderServices;
    use holon_frontend::reactive_view_model::ReactiveViewModel;
    use holon_gpui::geometry::BoundsRegistry;

    use super::support::FixtureView;
    use super::support::TestServices;

    /// One BMP glyph's advance under `NoopTextSystem` at `text_sm` (14px).
    const EM: f32 = 8.4;
    /// `rendered_text`'s left padding (`px(12.0)`).
    const TEXT_PAD_LEFT: f32 = 12.0;

    /// A `rendered_text` leaf VM: `content` in props, bare `id` + optional
    /// `marks` JSON in the data row (matching what the read projection feeds
    /// the builder in production).
    fn rendered_text_vm(id: &str, content: &str, marks: &[MarkSpan]) -> Arc<ReactiveViewModel> {
        let mut props: HashMap<String, Value> = HashMap::new();
        props.insert("content".into(), Value::String(content.into()));
        props.insert("field".into(), Value::String("content".into()));

        let mut data: DataRow = HashMap::new();
        data.insert("id".into(), Value::String(id.into()));
        if !marks.is_empty() {
            data.insert("marks".into(), Value::String(marks_to_json(marks)));
        }

        Arc::new(ReactiveViewModel::from_widget("rendered_text", props).with_entity(Arc::new(data)))
    }

    /// Open a headless window hosting `vm` and return the shared services +
    /// bounds registry plus the `VisualTestContext` for driving input.
    fn mount(
        cx: &mut TestAppContext,
        vm: Arc<ReactiveViewModel>,
    ) -> (Arc<TestServices>, BoundsRegistry, &mut VisualTestContext) {
        // The styled path pulls theme colors (`build_highlights` → `cx.theme()`),
        // so install the theme global before rendering. Harmless for the plain
        // path.
        cx.update(gpui_component::init);

        let services = TestServices::new();
        let bounds = BoundsRegistry::new();

        let (_view, vcx) = cx.add_window_view({
            let services = services.clone() as Arc<dyn BuilderServices>;
            let bounds = bounds.clone();
            move |_window, _cx| FixtureView::new(vm, services, bounds)
        });
        vcx.run_until_parked();

        (services, bounds, vcx)
    }

    /// Absolute window coordinates of the `n`-th glyph's centre within the sole
    /// `rendered_text` block. `n` is a glyph index (0-based); for all-ASCII
    /// content it equals the byte offset.
    fn glyph_center(bounds: &BoundsRegistry, n: usize) -> Point<Pixels> {
        let snap = holon_layout_testing::snapshot::snapshot_from_provider(bounds);
        let info = snap
            .of_type("rendered_text")
            .next()
            .expect("a rendered_text block should have painted");
        let x = info.x + TEXT_PAD_LEFT + n as f32 * EM + EM / 2.0;
        let y = info.y + info.height / 2.0;
        point(px(x), px(y))
    }

    /// Absolute window coordinates far to the right of the text (past line
    /// end).
    fn past_line_end(bounds: &BoundsRegistry) -> Point<Pixels> {
        let snap = holon_layout_testing::snapshot::snapshot_from_provider(bounds);
        let info = snap
            .of_type("rendered_text")
            .next()
            .expect("a rendered_text block should have painted");
        let x = info.x + info.width - 2.0;
        let y = info.y + info.height / 2.0;
        point(px(x), px(y))
    }

    /// Simulate a full left click (move → down → up) at `pos`.
    fn click(vcx: &mut VisualTestContext, pos: Point<Pixels>) {
        vcx.simulate_event(MouseMoveEvent {
            position: pos,
            ..Default::default()
        });
        vcx.simulate_event(MouseDownEvent {
            position: pos,
            button: MouseButton::Left,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            position: pos,
            button: MouseButton::Left,
            modifiers: Default::default(),
            click_count: 1,
        });
        vcx.run_until_parked();
    }

    fn caret_offset(services: &TestServices) -> Option<usize> {
        services.recorded_caret().map(|(_, offset)| offset)
    }

    /// Clicking mid-block on a plain (mark-less) block seeds the caret at the
    /// clicked offset — the long-standing dogfood bug (click ignored, caret
    /// landed elsewhere). Red on `main`: the click only ever called
    /// `set_focus`.
    #[gpui::test]
    fn click_in_plain_block_seeds_caret_at_click_offset(cx: &mut TestAppContext) {
        let block = "block:caret-plain";
        let vm = rendered_text_vm(block, "hello world", &[]);
        let (services, bounds, vcx) = mount(cx, vm);

        // Click inside the first glyph → offset 0.
        click(vcx, glyph_center(&bounds, 0));
        assert_eq!(
            caret_offset(&services),
            Some(0),
            "clicking the first glyph should seed caret at offset 0"
        );

        // Click inside the 7th glyph ('w' of "world", byte 6) → offset 6.
        click(vcx, glyph_center(&bounds, 6));
        assert_eq!(
            caret_offset(&services),
            Some(6),
            "clicking mid-block should seed the caret at the clicked offset, \
             not default to end-of-text"
        );

        // The seed targets the clicked block.
        assert_eq!(
            services.recorded_caret().map(|(b, _)| b),
            Some(EntityUri::from_raw(block)),
        );
    }

    /// UTF-8: the seeded offset is a byte offset, so a click after a multi-byte
    /// char lands past its bytes (not its scalar count). "café ok": 'é' is two
    /// bytes, so the 'o' is glyph index 5 but byte offset 6.
    #[gpui::test]
    fn click_after_multibyte_char_seeds_byte_offset(cx: &mut TestAppContext) {
        let vm = rendered_text_vm("block:caret-utf8", "café ok", &[]);
        let (services, bounds, vcx) = mount(cx, vm);

        // Glyph index 5 is 'o'; its byte offset is 6 (é = 2 bytes).
        click(vcx, glyph_center(&bounds, 5));
        assert_eq!(
            caret_offset(&services),
            Some(6),
            "caret seed must be a byte offset (6), not a scalar offset (5)"
        );
    }

    /// Clicking past the end of the line (in the block's whitespace to the
    /// right of the glyphs) is a disclosed degradation: `index_for_position`
    /// returns `Err`, so we fall back to plain `set_focus` (caret defaults to
    /// end-of-text on mount) rather than fabricating an offset.
    #[gpui::test]
    fn click_past_line_end_falls_back_to_plain_focus(cx: &mut TestAppContext) {
        let block = "block:caret-pastend";
        let vm = rendered_text_vm(block, "hi", &[]);
        let (services, bounds, vcx) = mount(cx, vm);

        click(vcx, past_line_end(&bounds));
        assert_eq!(
            caret_offset(&services),
            None,
            "a click past the glyphs must NOT arm a caret seed (fall back to end-of-text)"
        );
        assert_eq!(
            services.focused_block(),
            Some(EntityUri::from_raw(block)),
            "the block should still take focus even when the caret defaults to end"
        );
    }

    /// The styled path (block carries marks) plumbs the click offset the same
    /// way — a non-link click seeds the caret; the byte offset in the styled
    /// text equals the buffer offset (identity map, no delimiter chars today).
    #[gpui::test]
    fn click_in_styled_block_seeds_caret_at_click_offset(cx: &mut TestAppContext) {
        // "hello world" with "hello" bold — styled render path, no link.
        let marks = vec![MarkSpan::new(0, 5, InlineMark::Bold)];
        let vm = rendered_text_vm("block:caret-styled", "hello world", &marks);
        let (services, bounds, vcx) = mount(cx, vm);

        click(vcx, glyph_center(&bounds, 6));
        assert_eq!(
            caret_offset(&services),
            Some(6),
            "clicking a non-link span in a styled block should seed the caret at the offset"
        );
    }

    /// An `External` link target is a WEB address, not an entity: clicking it
    /// hands the URL to the platform, and must never become a
    /// `navigation.focus` whose `block_id` is that URL — the dispatch that
    /// blanked the whole main panel with no ERROR line (BugFunnel 2026-08-08,
    /// task #17). `cx.opened_url()` reads gpui's `TestPlatform` recorder, so
    /// the assertion observes the open WITHOUT launching a real browser.
    #[gpui::test]
    fn clicking_an_external_link_opens_the_url_instead_of_navigating(cx: &mut TestAppContext) {
        // "see the example site" — "example site" (offsets 8..20) is the link.
        let marks = vec![MarkSpan::new(
            8,
            20,
            InlineMark::Link {
                target: EntityRef::External {
                    url: "https://example.com".to_string(),
                },
                label: "example site".to_string(),
            },
        )];
        let vm = rendered_text_vm("block:link-external", "see the example site", &marks);
        let (services, bounds, vcx) = mount(cx, vm);

        // Glyph 10 sits inside "example site".
        click(vcx, glyph_center(&bounds, 10));

        let nav_intents: Vec<_> = services
            .recorded_intents()
            .into_iter()
            .filter(|i| i.entity_name == "navigation")
            .collect();
        assert!(
            nav_intents.is_empty(),
            "an external URL must never be dispatched as a navigation target; got {nav_intents:?}"
        );
        assert_eq!(
            vcx.opened_url(),
            Some("https://example.com".to_string()),
            "clicking an external link should hand the URL to the platform opener"
        );
    }

    /// The control for the test above: a link to a REGISTERED entity scheme
    /// still navigates, and opens no URL. Pins that the external routing fix
    /// did not disarm entity links.
    #[gpui::test]
    fn clicking_an_entity_link_still_navigates(cx: &mut TestAppContext) {
        let target = "block:bbbb2222-0000-4000-8000-00000000000a";
        let marks = vec![MarkSpan::new(
            4,
            9,
            InlineMark::Link {
                target: EntityRef::Scheme {
                    raw: target.to_string(),
                },
                label: "other".to_string(),
            },
        )];
        let vm = rendered_text_vm("block:link-entity", "see other now", &marks);
        let (services, bounds, vcx) = mount(cx, vm);

        click(vcx, glyph_center(&bounds, 6));

        let nav_intents: Vec<_> = services
            .recorded_intents()
            .into_iter()
            .filter(|i| i.entity_name == "navigation")
            .collect();
        assert_eq!(
            nav_intents.len(),
            1,
            "a registered entity link should dispatch exactly one navigation intent"
        );
        assert_eq!(
            nav_intents[0]
                .params
                .get("block_id")
                .and_then(|v| v.as_string()),
            Some(target),
            "the navigation must target the entity URI"
        );
        assert_eq!(
            vcx.opened_url(),
            None,
            "an entity link must not open a browser"
        );
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
