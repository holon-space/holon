//! Windowed property for deferred `live_block` content: every block paints its
//! placeholder until `DeliverBlockContent` pushes its content, and from then on
//! paints that content — exactly the delivered blocks, no other.
//!
//! The transitions come from the shared `DeliverBlockContent` generator over
//! the blocks this case mounts; delivery goes through the shared
//! `apply_to_sut` into `LiveBlockSink`, which pushes the loaded tree down the
//! block's structural-change stream to the real `ReactiveShell`. The headless
//! keystone renders no deferred blocks, so this suite vouches for the kind via
//! `attest_drew`.
//!
//! Run: `cargo nextest run -p holon-gpui --test
//! main deliver_block_content_windowed_pbt::` ⚠ `--test-threads=1` mandatory
//! (gpui `TestAppContext` is not parallel-safe).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::AnyWindowHandle;
use gpui::AppContext as _;
use gpui::Bounds;
use gpui::Point;
use gpui::TestAppContext;
use gpui::WindowBounds;
use gpui::WindowHandle;
use gpui::WindowOptions;
use gpui::px;
use gpui::size;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::geometry::BoundsRegistry;
use holon_integration_tests::pbt::alphabet_coverage;
use holon_layout_testing::BlockTreeRegistry;
use holon_layout_testing::BlockTreeThunk;
use holon_layout_testing::BoundsSnapshot;
use holon_layout_testing::DeliverBlockContent;
use holon_layout_testing::LayoutRef;
use holon_layout_testing::LayoutRefState;
use holon_layout_testing::LayoutSut;
use holon_layout_testing::LiveBlockSink;
use holon_layout_testing::blueprint::BlockHandle;
use holon_layout_testing::blueprint::DrawerHandle;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use support::ReactiveFixtureView;
use validated::Validated;

use crate::support;

const KIND: &str = "DeliverBlockContent";
const PLACEHOLDER_MODE: &str = "placeholder";
const LOADED_MODE: &str = "loaded";

/// The mounted deferred blocks and which of them have received content.
#[derive(Clone, Debug)]
struct DeferredBlocks {
    ids: Vec<String>,
    delivered: BTreeSet<String>,
}

impl LayoutRefState for DeferredBlocks {
    fn switchable_handles(&self) -> &[BlockHandle] {
        &[]
    }
    fn drawer_handles(&self) -> &[DrawerHandle] {
        &[]
    }
    /// Every mounted block stays a delivery target: a repeated delivery must
    /// leave the content painted.
    fn deferred_block_ids(&self) -> Vec<String> {
        self.ids.clone()
    }
    fn current_view_mode(&self, _: &str) -> Option<&str> {
        None
    }
    fn drawer_is_open(&self, _: &str) -> bool {
        false
    }
}

struct DeferredBlocksMachine;

impl ReferenceStateMachine for DeferredBlocksMachine {
    type State = DeferredBlocks;
    type Transition = DeliverBlockContent;

    fn init_state() -> BoxedStrategy<Self::State> {
        (1..=4usize)
            .prop_map(|n| DeferredBlocks {
                ids: (0..n)
                    .map(|i| EntityUri::block(&format!("deferred-{i}")).to_string())
                    .collect(),
                delivered: BTreeSet::new(),
            })
            .boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        match DeliverBlockContent::weighted_generator(&LayoutRef::new(state)) {
            Validated::Good((_, strategy)) => {
                alphabet_coverage::note_offered(KIND);
                strategy
            }
            Validated::Fail(reasons) => {
                panic!("{KIND} declined a state with mounted deferred blocks: {reasons:?}")
            }
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        state.delivered.insert(transition.block_id.clone());
        state
    }
}

/// Pushes a block's loaded mode down its structural-change stream.
struct RegistrySink<'a>(&'a BlockTreeRegistry);

impl LiveBlockSink for RegistrySink<'_> {
    fn deliver_block_content_loaded(&mut self, block_id: &str) {
        self.0.set_active(block_id, LOADED_MODE);
    }
}

fn placeholder_row(block_id: &str) -> String {
    format!("{block_id}-placeholder")
}

fn content_row(block_id: &str) -> String {
    format!("{block_id}-content")
}

fn content_text(block_id: &str) -> String {
    format!("content of {block_id}")
}

/// A data-bound `text(col("content"))` row, which the text builder tracks with
/// its row id and painted string.
fn text_row(row_id: String, label: String) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(row_id));
    data.insert("content".to_string(), Value::String(label.clone()));
    let mut props = HashMap::new();
    props.insert("content".to_string(), Value::String(label));
    props.insert("field".to_string(), Value::String("content".to_string()));
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = Mutable::new(Arc::new(data)).read_only();
    vm
}

fn register_deferred(registry: &BlockTreeRegistry, block_id: &str) {
    let placeholder: BlockTreeThunk = {
        let row = placeholder_row(block_id);
        Arc::new(move || text_row(row.clone(), "loading".to_string()))
    };
    let loaded: BlockTreeThunk = {
        let (row, text) = (content_row(block_id), content_text(block_id));
        Arc::new(move || text_row(row.clone(), text.clone()))
    };
    registry.register(
        block_id,
        vec![
            (PLACEHOLDER_MODE.to_string(), placeholder),
            (LOADED_MODE.to_string(), loaded),
        ],
        0,
    );
}

fn columns_of_live_blocks(ids: &[String]) -> Arc<ReactiveViewModel> {
    let blocks = ids
        .iter()
        .map(|id| ReactiveViewModel::live_block(EntityUri::parse(id).expect("a block uri")))
        .collect();
    Arc::new(ReactiveViewModel {
        collection: Some(Arc::new(ReactiveView::new_static_with_layout(
            blocks,
            CollectionVariant::columns(4.0),
        ))),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    })
}

/// The test platform draws a window only when asked; a shell's `notify` alone
/// leaves the last frame's bounds in place.
fn settle_and_draw(cx: &mut TestAppContext, window: WindowHandle<ReactiveFixtureView>) {
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    let window: AnyWindowHandle = window.into();
    cx.update(|cx| window.update(cx, |_, window, cx| window.draw(cx).clear(cx)))
        .expect("the case window is still open");
}

fn painted_text(snap: &BoundsSnapshot, row_id: &str) -> Option<String> {
    snap.of_type("text")
        .find(|i| i.height > 0.0 && i.entity_id.as_deref() == Some(row_id))
        .map(|i| i.displayed_text.as_deref().unwrap_or_default().to_string())
}

/// Each delivered block paints its content and not its placeholder; each
/// undelivered one the reverse.
fn check_painted(snap: &BoundsSnapshot, state: &DeferredBlocks, after: &str) -> Result<(), String> {
    for id in &state.ids {
        let placeholder = painted_text(snap, &placeholder_row(id));
        let content = painted_text(snap, &content_row(id));
        let delivered = state.delivered.contains(id);
        let expected_content = delivered.then(|| content_text(id));
        if content != expected_content || placeholder.is_some() == delivered {
            return Err(format!(
                "after {after}: {id} (delivered: {delivered}) painted content {content:?} and \
                 placeholder {placeholder:?}; expected content {expected_content:?} and the \
                 placeholder {}",
                if delivered { "gone" } else { "painted" }
            ));
        }
    }
    Ok(())
}

/// Mount the case's blocks in a fresh window, then apply each delivery and
/// judge the painted frame after it.
fn drive_case(
    cx: &mut TestAppContext,
    initial: DeferredBlocks,
    transitions: Vec<DeliverBlockContent>,
) -> Result<(), String> {
    let registry = Arc::new(BlockTreeRegistry::new());
    for id in &initial.ids {
        register_deferred(&registry, id);
    }
    let services: Arc<dyn BuilderServices> =
        support::TestServices::with_registry_quiescent(registry.clone());
    let root = columns_of_live_blocks(&initial.ids);
    let window_size = size(px(900.0), px(600.0));
    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: window_size,
                })),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| {
                    ReactiveFixtureView::with_services_and_bounds(
                        root,
                        services,
                        window_size,
                        bounds_for_view,
                    )
                })
            },
        )
        .expect("open_window failed")
    });
    settle_and_draw(cx, window);

    let snapshot = || holon_layout_testing::snapshot::snapshot_from_provider(&bounds);
    let mut state = initial;
    let mut verdict = check_painted(&snapshot(), &state, "mount");
    for transition in &transitions {
        if verdict.is_err() {
            break;
        }
        futures::executor::block_on(transition.apply_to_sut(
            &LayoutRef::new(&state),
            &mut LayoutSut::new(&mut RegistrySink(&registry)),
        ));
        alphabet_coverage::note_drawn(KIND);
        state = DeferredBlocksMachine::apply(state, transition);
        settle_and_draw(cx, window);
        verdict = check_painted(&snapshot(), &state, &format!("{transition:?}"));
    }
    window
        .update(cx, |_, window, _| window.remove_window())
        .expect("the case window is still open");
    verdict
}

#[gpui::test]
fn deferred_live_blocks_paint_exactly_the_delivered_content(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let cases: u32 = std::env::var("PROPTEST_CASES")
        .ok()
        .map(|s| s.parse().expect("PROPTEST_CASES is a number"))
        .unwrap_or(16);
    let mut runner = TestRunner::new(Config {
        cases,
        ..Config::default()
    });
    let cx = RefCell::new(cx);
    let result = runner.run(
        &DeferredBlocksMachine::sequential_strategy(1..=6),
        |(initial, transitions, _)| {
            drive_case(&mut cx.borrow_mut(), initial, transitions).map_err(TestCaseError::fail)
        },
    );
    if let Err(e) = result {
        panic!("deferred live-block delivery failed (shrunk): {e}");
    }
    alphabet_coverage::attest_drew(KIND);
}
