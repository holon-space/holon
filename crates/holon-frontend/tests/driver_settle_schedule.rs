//! A collection driver over a delivering row provider holds the UI watermark
//! (`UiWatermark::owed`) until its `items` show every delivered batch and the
//! inputs it re-interprets for. A drawn schedule of engine batches (also with
//! a repeated or zero seq), profile, template and space changes, and view
//! starts and drops, some of them while every tokio worker is held, must leave
//! `owed()` empty with no further input, and an empty `owed()` must mean the
//! items already show the rows.
//!
//! Run: `cargo nextest run -p holon-frontend driver_settle_schedule::`

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use futures_signals::signal::Mutable;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::entity_profile::ProfileCache;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_api::render_types::RenderProfile;
use holon_api::streaming::Change;
use holon_api::streaming::ChangeOrigin;
use holon_api::widget_spec::DataRow;
use holon_api::widget_spec::EnrichedRow;
use holon_frontend::AvailableSpace;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveRenderedRows;
use holon_frontend::reactive_view::CollectionConfig;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::render_epoch::UiWatermark;
use holon_frontend::render_interpreter::RenderInterpreter;
use proptest::prelude::*;

const PROVIDERS: usize = 2;
const ROWS: u8 = 4;
const CONTENTS: [&str; 3] = ["", "x", "y z"];
const LANES: [&str; 3] = ["", "todo", "done"];
/// Longer than any drawn schedule needs on a loaded host; a driver still owed
/// after it with no input pending is stuck.
const QUIET_DEADLINE: Duration = Duration::from_secs(10);

/// Services with a watermark, a profile cache the test changes, and an
/// interpret that takes `interpret_delay`, so batches arrive while a driver
/// is still applying the previous one.
#[derive(Clone)]
struct WatchedServices {
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    watermark: Arc<UiWatermark>,
    profiles: Mutable<Arc<ProfileCache>>,
    /// Every cache ever published; a cache's generation is its index, so a row
    /// names the generation of the one cache it read.
    published: Arc<Mutex<Vec<Arc<ProfileCache>>>>,
    focus: Mutable<Option<EntityUri>>,
    /// The row whose next interpret runs with an input moved away and back,
    /// as another thread can do while a worker interprets that row.
    trip: Arc<Mutex<Option<(EntityUri, ArmedTrip)>>>,
    interpret_delay: Duration,
    rt_handle: tokio::runtime::Handle,
    link_classifier: Arc<holon_api::link_parser::LinkTargetClassifier>,
}

#[derive(Clone)]
enum ArmedTrip {
    Space(Mutable<Option<AvailableSpace>>, Option<AvailableSpace>),
    Profile,
    Focus(Option<EntityUri>),
}

impl WatchedServices {
    fn take_trip(&self, ctx: &RenderContext) -> Option<ArmedTrip> {
        let Some(Value::String(id)) = ctx.row().get("id") else {
            return None;
        };
        let id = EntityUri::parse(id).expect("a drawn row id parses");
        let mut armed = self.trip.lock().unwrap();
        if armed.as_ref().is_some_and(|(row, _)| *row == id) {
            armed.take().map(|(_, trip)| trip)
        } else {
            None
        }
    }

    fn generation_of(&self, cache: &Arc<ProfileCache>) -> u64 {
        let mut published = self.published.lock().unwrap();
        let index = match published.iter().position(|p| Arc::ptr_eq(p, cache)) {
            Some(index) => index,
            None => {
                published.push(cache.clone());
                published.len() - 1
            }
        };
        index as u64
    }
}

impl BuilderServices for WatchedServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        std::thread::sleep(self.interpret_delay);
        match self.take_trip(ctx) {
            None => self.interpreter.interpret(expr, ctx, self),
            Some(ArmedTrip::Space(space, to)) => {
                let before = space.replace(to);
                let model = self.interpreter.interpret(expr, ctx, self);
                space.set(before);
                model
            }
            Some(ArmedTrip::Profile) => {
                // Production publishes a fresh cache per rebuild, never the
                // one it left; a trip leaves the empty start cache.
                self.profiles.set(Arc::new(ProfileCache::empty()));
                let model = self.interpreter.interpret(expr, ctx, self);
                self.profiles.set(Arc::new(ProfileCache::empty()));
                model
            }
            Some(ArmedTrip::Focus(to)) => {
                let before = self.focus.replace(to);
                let model = self.interpreter.interpret(expr, ctx, self);
                self.focus.set(before);
                model
            }
        }
    }

    fn value_fn_lookup<'a>(
        &'a self,
        ctx: &'a RenderContext,
    ) -> Box<dyn holon_api::render_eval::ValueFnLookup + 'a> {
        self.interpreter.value_fn_lookup(self, ctx)
    }

    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        Arc::new(self.clone())
    }

    fn ui_watermark(&self) -> Option<Arc<UiWatermark>> {
        Some(self.watermark.clone())
    }

    fn profile_signal(&self) -> Mutable<Arc<ProfileCache>> {
        self.profiles.clone()
    }

    fn focused_block_mutable(&self) -> Option<Mutable<Option<EntityUri>>> {
        Some(self.focus.clone())
    }

    /// Every row resolves to one profile whose render names the profile
    /// generation and whether the row is focused, read when the row is
    /// interpreted, as a production `is_focused` variant is.
    fn resolve_profile(&self, row: &DataRow) -> Option<RenderProfile> {
        let id = match row.get("id") {
            Some(Value::String(id)) => EntityUri::parse(id).expect("a drawn row id parses"),
            other => panic!("a drawn row carries a string id, got {other:?}"),
        };
        let focused = self.focus.get_cloned().as_ref() == Some(&id);
        Some(RenderProfile {
            name: "drawn".to_string(),
            render: profile_render(self.generation_of(&self.profiles.get_cloned()), focused),
            operations: Vec::new(),
            variants: Vec::new(),
        })
    }

    fn write_tier_admits(&self, _: &EntityUri) -> bool {
        true
    }

    fn get_block_data(&self, _: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        (call("table", Vec::new()), Vec::new())
    }

    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        &self.link_classifier
    }

    fn watch_query(
        &self,
        _: &str,
        _: holon_api::QueryLanguage,
        _: Option<holon_frontend::QueryContext>,
    ) -> anyhow::Result<holon_api::EnrichedChangeStream> {
        anyhow::bail!("WatchedServices drives row providers directly, no query is watched")
    }

    fn widget_state(&self, _: &str) -> holon_frontend::WidgetState {
        holon_frontend::WidgetState::default()
    }

    fn dispatch_intent(&self, _: holon_frontend::operations::OperationIntent) {}

    fn dispatch_intent_awaitable(
        &self,
        _: holon_frontend::operations::OperationIntent,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<holon_core::Delivery>> + Send + 'static,
        >,
    > {
        Box::pin(std::future::ready(Ok(holon_core::Delivery::Unproven {
            detail: "WatchedServices: test double, no delivery to prove".to_string(),
        })))
    }

    fn present_op(
        &self,
        _: holon_api::render_types::OperationDescriptor,
        _: HashMap<String, Value>,
    ) {
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.rt_handle.clone()
    }

    fn search_link_candidates(
        &self,
        _: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        Box::pin(async { anyhow::bail!("WatchedServices searches no links") })
    }
}

fn call(name: &str, args: Vec<RenderExpr>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args: args
            .into_iter()
            .map(|value| Arg { name: None, value })
            .collect(),
    }
}

fn text_of(value: RenderExpr) -> RenderExpr {
    call("text", vec![value])
}

fn content_column() -> RenderExpr {
    RenderExpr::ColumnRef {
        name: "content".to_string(),
    }
}

fn profile_label(generation: u64, focused: bool) -> String {
    format!("p{generation}{}", if focused { "*" } else { "" })
}

fn profile_render(generation: u64, focused: bool) -> RenderExpr {
    call(
        "row",
        vec![
            text_of(RenderExpr::Literal {
                value: Value::String(profile_label(generation, focused)),
            }),
            text_of(content_column()),
        ],
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Flat,
    Grouped,
    /// A `tree` layout of rows without parents: the tree driver.
    Tree,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Template {
    /// `text(col("content"))`: the row's content only.
    Content,
    /// `text(col("lane"))`: the same widget as `Content`, other props.
    Lane,
    /// `render_entity()`: the row through its profile.
    Entity,
    /// `if_space(600, text("narrow"), text("wide"))`: the space the row was
    /// interpreted for.
    Width,
}

const NARROW_BELOW: u16 = 600;

impl Template {
    fn expr(self) -> RenderExpr {
        match self {
            Template::Content => text_of(content_column()),
            Template::Lane => text_of(RenderExpr::ColumnRef {
                name: "lane".to_string(),
            }),
            Template::Entity => call("render_entity", Vec::new()),
            Template::Width => {
                let literal = |text: &str| {
                    text_of(RenderExpr::Literal {
                        value: Value::String(text.to_string()),
                    })
                };
                call(
                    "if_space",
                    vec![
                        RenderExpr::Literal {
                            value: Value::Float(f64::from(NARROW_BELOW)),
                        },
                        literal("narrow"),
                        literal("wide"),
                    ],
                )
            }
        }
    }

    fn texts(
        self,
        row: &ModelRow,
        profile_generation: u64,
        focused: bool,
        width: Option<u16>,
    ) -> Vec<String> {
        match self {
            Template::Content => vec![row.content.to_string()],
            Template::Lane => vec![row.lane.to_string()],
            Template::Entity => vec![
                profile_label(profile_generation, focused),
                row.content.to_string(),
            ],
            Template::Width if width.is_some_and(|w| w < NARROW_BELOW) => {
                vec!["narrow".to_string()]
            }
            Template::Width => vec!["wide".to_string()],
        }
    }

    fn widget(self) -> &'static str {
        match self {
            Template::Content | Template::Lane => "text",
            Template::Entity => "row",
            Template::Width => "if_space",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Seq {
    /// A seq above every seq delivered so far.
    Next,
    /// The seq this provider delivered last: a batch with no new seq.
    Repeat,
    /// A snapshot batch of a query watcher.
    Zero,
}

#[derive(Clone, Copy, Debug)]
enum RowChange {
    Upsert { row: u8, content: u8, lane: u8 },
    Delete { row: u8 },
}

#[derive(Clone, Debug)]
enum Step {
    Batch {
        provider: u8,
        changes: Vec<RowChange>,
        seq: Seq,
    },
    /// A new profile cache: every row resolves a new profile.
    ProfileChange,
    /// The same profile cache Arc published again.
    ProfileRepublish,
    /// The cache and generation from before the last `ProfileChange`, published
    /// again.
    ProfileRevert,
    /// The focused block becomes row `row` of every provider, or none.
    SetFocus {
        row: Option<u8>,
    },
    SetTemplate {
        view: u8,
        template: Template,
    },
    SetSpace {
        view: u8,
        width: Option<u16>,
    },
    /// While row `row` is next interpreted, the input moves to `to` and back.
    TripInsideRow {
        row: u8,
        to: Trip,
    },
    StartView(ViewSpec),
    DropView(u8),
    HoldWorkers,
    ReleaseWorkers,
    /// The test thread sleeps through `interprets` interprets and half of the
    /// next, so the next input lands inside a started driver's interpret pass.
    Pause {
        interprets: u8,
    },
}

#[derive(Clone, Copy, Debug)]
enum Trip {
    Space {
        view: u8,
        width: Option<u16>,
    },
    /// A new profile cache, then the one from before.
    Profile,
    Focus {
        row: Option<u8>,
    },
}

#[derive(Clone, Copy, Debug)]
struct ViewSpec {
    kind: Kind,
    provider: u8,
    template: Template,
}

#[derive(Clone, Debug)]
struct Case {
    workers: usize,
    interpret_delay: Duration,
    views: Vec<ViewSpec>,
    steps: Vec<Step>,
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![Just(Kind::Flat), Just(Kind::Grouped)]
}

fn template() -> impl Strategy<Value = Template> {
    prop_oneof![
        Just(Template::Content),
        Just(Template::Lane),
        Just(Template::Entity),
        Just(Template::Width)
    ]
}

fn view_spec() -> impl Strategy<Value = ViewSpec> {
    (kind(), 0..PROVIDERS as u8, template()).prop_map(|(kind, provider, template)| ViewSpec {
        kind,
        provider,
        template,
    })
}

fn row_change() -> impl Strategy<Value = RowChange> {
    prop_oneof![
        3 => (0..ROWS, 0..CONTENTS.len() as u8, 0..LANES.len() as u8)
            .prop_map(|(row, content, lane)| RowChange::Upsert { row, content, lane }),
        1 => (0..ROWS).prop_map(|row| RowChange::Delete { row }),
    ]
}

fn seq() -> impl Strategy<Value = Seq> {
    prop_oneof![
        3 => Just(Seq::Next),
        1 => Just(Seq::Repeat),
        1 => Just(Seq::Zero),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => (0..PROVIDERS as u8, prop::collection::vec(row_change(), 0..4), seq())
            .prop_map(|(provider, changes, seq)| Step::Batch { provider, changes, seq }),
        1 => Just(Step::ProfileChange),
        1 => Just(Step::ProfileRepublish),
        1 => prop::option::of(0..ROWS).prop_map(|row| Step::SetFocus { row }),
        1 => (any::<u8>(), template())
            .prop_map(|(view, template)| Step::SetTemplate { view, template }),
        1 => (any::<u8>(), prop::option::of(prop_oneof![Just(320u16), Just(800), Just(1400)]))
            .prop_map(|(view, width)| Step::SetSpace { view, width }),
        1 => view_spec().prop_map(Step::StartView),
        1 => any::<u8>().prop_map(Step::DropView),
        2 => Just(Step::HoldWorkers),
        2 => Just(Step::ReleaseWorkers),
        1 => (1u8..=3).prop_map(|interprets| Step::Pause { interprets }),
    ]
}

fn case() -> impl Strategy<Value = Case> {
    (
        1usize..=3,
        prop_oneof![Just(0u64), Just(300), Just(3_000)],
        prop::collection::vec(view_spec(), 1..=3),
        prop::collection::vec(step(), 1..=24),
    )
        .prop_map(|(workers, delay_us, views, steps)| Case {
            workers,
            interpret_delay: Duration::from_micros(delay_us),
            views,
            steps,
        })
}

/// Every worker of the runtime blocks in a task until the hold drops, so no
/// driver runs and no aborted driver is dropped meanwhile.
struct Hold {
    released: Arc<(Mutex<bool>, Condvar)>,
}

impl Hold {
    fn all_workers(rt: &tokio::runtime::Runtime, workers: usize) -> Self {
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(AtomicUsize::new(0));
        let spawn_one = || {
            let released = released.clone();
            let entered = entered.clone();
            rt.spawn(async move {
                entered.fetch_add(1, Ordering::AcqRel);
                let (lock, cvar) = &*released;
                let mut done = lock.lock().unwrap();
                while !*done {
                    done = cvar.wait(done).unwrap();
                }
            });
        };
        for _ in 0..workers {
            spawn_one();
        }
        let deadline = Instant::now() + QUIET_DEADLINE;
        let mut top_up = Instant::now();
        while entered.load(Ordering::Acquire) < workers {
            assert!(
                Instant::now() < deadline,
                "only {} of {workers} tokio workers entered the hold",
                entered.load(Ordering::Acquire)
            );
            // A worker that took a hold task can carry the next one in its own
            // queue; a fresh spawn wakes an idle worker.
            if top_up.elapsed() > Duration::from_millis(20) {
                spawn_one();
                top_up = Instant::now();
            }
            std::thread::sleep(Duration::from_micros(100));
        }
        Self { released }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.released;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }
}

struct LiveView {
    view: ReactiveView,
    spec: ViewSpec,
    /// The template the view was built with; a flat driver keeps using it.
    built_template: Template,
    width: Option<u16>,
    /// The width the driver last rebuilt for, while the test knows it: from
    /// the start or a settled check until a space change with a free worker.
    rebuilt_width: Option<Option<u16>>,
    /// The template the driver last re-interpreted for, known as the width is.
    applied_template: Option<Template>,
    /// An input reached the driver since the last settled check.
    touched: bool,
    /// Why the rows of this view are no longer judged, if they are not.
    unjudged: Option<&'static str>,
}

/// The flat driver re-interprets a row on a batch, and every row on a
/// profile, space or focus rebuild, with the template it was built with.
const RETEMPLATE_REVERTS: &str = "a flat driver renders later rows and rebuilds with the item \
     template it was built with, not the one set_template gave it";
/// The flat template driver updates props in place and keeps the widget.
const RETEMPLATE_KEEPS_WIDGET: &str =
    "set_template to a template of another widget keeps the old widget and its children";
/// The grouped driver re-renders its cards only on a row event.
const GROUPED_KEEPS_PROFILE: &str =
    "the grouped driver does not re-render its cards when the profile cache changes";
/// The grouped driver re-renders its cards only on a row event.
const GROUPED_KEEPS_FOCUS: &str =
    "the grouped driver does not re-render its cards when the focused block changes";
/// The grouped driver reads its space only when a row event re-renders a card.
const GROUPED_KEEPS_SPACE: &str =
    "the grouped driver does not re-render its cards when its space changes";

#[derive(Clone, Debug, PartialEq)]
struct ModelRow {
    content: &'static str,
    lane: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Oracle {
    /// Stops judging a view's rows once one of the defects named above has
    /// reached it; `owed()` is still judged.
    SkipKnownDefects,
    Full,
}

/// Field order is drop order: the hold releases the workers before the views
/// stop and the runtime shuts down.
struct Run {
    hold: Option<Hold>,
    views: Vec<LiveView>,
    services: WatchedServices,
    providers: Vec<Arc<ReactiveRenderedRows>>,
    model: Vec<BTreeMap<u8, ModelRow>>,
    last_seq: Vec<u64>,
    next_seq: u64,
    profile_before_change: Option<Arc<ProfileCache>>,
    /// A space trip armed before any view exists: the next started view
    /// takes it.
    pending_space_trip: Option<(EntityUri, Option<AvailableSpace>)>,
    /// The focus the drivers last re-interpreted for, while the test knows
    /// it: from the start or a settled check until a focus change with a
    /// free worker.
    applied_focus: Option<Option<u8>>,
    workers: usize,
    oracle: Oracle,
    rt: tokio::runtime::Runtime,
}

fn label(provider: usize) -> String {
    format!("rows-{provider}")
}

fn row_id(row: u8) -> String {
    format!("block:r{row}")
}

fn remote() -> ChangeOrigin {
    ChangeOrigin::Remote {
        operation_id: None,
        trace_id: None,
    }
}

fn space(width: u16) -> AvailableSpace {
    AvailableSpace {
        width_px: f32::from(width),
        height_px: 600.0,
        width_physical_px: f32::from(width) * 2.0,
        height_physical_px: 1200.0,
        scale_factor: 2.0,
    }
}

/// Every text leaf under `node`, in tree order.
fn texts(node: &ReactiveViewModel) -> Vec<String> {
    if node.widget_name().as_deref() == Some("text") {
        return vec![node.prop_str("content").unwrap_or_default()];
    }
    let mut out: Vec<String> = node.children.iter().flat_map(|c| texts(c)).collect();
    if let Some(view) = &node.collection {
        out.extend(view.items.lock_ref().iter().flat_map(|c| texts(c)));
    }
    if let Some(slot) = &node.slot {
        out.extend(texts(&slot.content.lock_ref()));
    }
    out
}

impl LiveView {
    fn touch(&mut self) {
        self.touched = true;
        if self.spec.kind == Kind::Flat && self.spec.template != self.built_template {
            self.unjudged.get_or_insert(RETEMPLATE_REVERTS);
        }
    }
}

impl Run {
    fn new(case: &Case, oracle: Oracle) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(case.workers)
            .enable_all()
            .build()
            .expect("test runtime");
        let services = WatchedServices {
            interpreter: Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            watermark: UiWatermark::new(None),
            profiles: Mutable::new(Arc::new(ProfileCache::empty())),
            published: Arc::default(),
            focus: Mutable::new(None),
            trip: Arc::default(),
            interpret_delay: case.interpret_delay,
            rt_handle: rt.handle().clone(),
            link_classifier: Arc::new(holon_api::link_parser::LinkTargetClassifier::default()),
        };
        let providers = (0..PROVIDERS)
            .map(|p| {
                let rows = ReactiveRenderedRows::labeled(label(p));
                rows.set_generation(1);
                Arc::new(rows)
            })
            .collect();
        let mut run = Self {
            hold: None,
            views: Vec::new(),
            services,
            providers,
            model: vec![BTreeMap::new(); PROVIDERS],
            last_seq: vec![0; PROVIDERS],
            next_seq: 0,
            profile_before_change: None,
            pending_space_trip: None,
            applied_focus: Some(None),
            workers: case.workers,
            oracle,
            rt,
        };
        for spec in &case.views {
            run.start_view(*spec);
        }
        run
    }

    fn start_view(&mut self, spec: ViewSpec) {
        let source: Arc<dyn holon_api::ReactiveRowProvider> =
            self.providers[spec.provider as usize].clone();
        let view = match spec.kind {
            Kind::Flat | Kind::Tree => ReactiveView::new_collection(
                CollectionConfig {
                    layout: CollectionVariant::from_name(
                        if spec.kind == Kind::Tree {
                            "tree"
                        } else {
                            "list"
                        },
                        0.0,
                    )
                    .expect("`list` and `tree` are registered layouts"),
                    item_template: spec.template.expr(),
                    sort_key: None,
                    virtual_child: None,
                    rules: Vec::new(),
                    context_root_id: None,
                },
                source,
                None,
                None,
            ),
            Kind::Grouped => ReactiveView::new_grouped(
                CollectionVariant::from_name("board", 0.0).expect("`board` is a registered layout"),
                source,
                spec.template.expr(),
                "lane".to_string(),
                "No lane".to_string(),
                Vec::new(),
                None,
                None,
                Vec::new(),
            ),
        };
        if let Some((row, to)) = self.pending_space_trip.take() {
            let mutable = view.space_mutable().expect("a collection has a space");
            *self.services.trip.lock().unwrap() =
                Some((row, ArmedTrip::Space(mutable.clone(), to)));
        }
        view.start(Arc::new(self.services.clone()), self.rt.handle());
        self.views.push(LiveView {
            view,
            spec,
            built_template: spec.template,
            width: None,
            rebuilt_width: Some(None),
            applied_template: Some(spec.template),
            touched: true,
            unjudged: None,
        });
    }

    fn require_owed_while_held(&self, provider: usize, what: &str) -> Result<(), String> {
        let needle = format!("driver of {}", label(provider));
        let owed = self.services.watermark.owed();
        if self.hold.is_some() && !owed.iter().any(|line| line.contains(&needle)) {
            return Err(format!(
                "{what} while every tokio worker is held, but owed() names no driver of {}: {owed:?}",
                label(provider),
            ));
        }
        Ok(())
    }

    fn apply(&mut self, step: &Step) -> Result<(), String> {
        match step {
            Step::Batch {
                provider,
                changes,
                seq,
            } => {
                let p = *provider as usize;
                for change in changes {
                    self.apply_row_change(p, *change);
                }
                let seq = match seq {
                    Seq::Next => {
                        self.next_seq += 1;
                        self.last_seq[p] = self.next_seq;
                        self.next_seq
                    }
                    Seq::Repeat => self.last_seq[p],
                    Seq::Zero => 0,
                };
                self.providers[p].delivered(seq);
                let mut viewed = false;
                for v in self
                    .views
                    .iter_mut()
                    .filter(|v| v.spec.provider == *provider)
                {
                    v.touch();
                    viewed = true;
                }
                if viewed {
                    self.require_owed_while_held(
                        p,
                        &format!("a batch with seq {seq} was delivered"),
                    )?;
                }
            }
            Step::ProfileChange | Step::ProfileRepublish | Step::ProfileRevert => {
                let cache = match step {
                    Step::ProfileChange => {
                        self.profile_before_change = Some(self.services.profiles.get_cloned());
                        Arc::new(ProfileCache::empty())
                    }
                    Step::ProfileRevert => match self.profile_before_change.take() {
                        Some(cache) => cache,
                        None => return Ok(()),
                    },
                    _ => self.services.profiles.get_cloned(),
                };
                self.services.profiles.set(cache);
                let changed = !matches!(step, Step::ProfileRepublish);
                for v in &mut self.views {
                    match v.spec.kind {
                        // A republish of the same cache rebuilds too, but owes nothing.
                        Kind::Flat | Kind::Tree => v.touch(),
                        Kind::Grouped if changed && v.spec.template == Template::Entity => {
                            v.unjudged.get_or_insert(GROUPED_KEEPS_PROFILE);
                        }
                        Kind::Grouped => {}
                    }
                }
                if changed {
                    for v in self.views.iter().filter(|v| v.spec.kind != Kind::Grouped) {
                        self.require_owed_while_held(
                            v.spec.provider as usize,
                            "the profile cache changed under a flat view",
                        )?;
                    }
                }
            }
            Step::SetFocus { row } => {
                self.services
                    .focus
                    .set(row.map(|r| EntityUri::parse(&row_id(r)).expect("a drawn row id parses")));
                let owes = self.applied_focus.is_some_and(|applied| applied != *row);
                if self.hold.is_none() {
                    self.applied_focus = None;
                }
                for v in &mut self.views {
                    match v.spec.kind {
                        Kind::Flat | Kind::Tree => v.touch(),
                        Kind::Grouped if v.spec.template == Template::Entity => {
                            v.unjudged.get_or_insert(GROUPED_KEEPS_FOCUS);
                        }
                        Kind::Grouped => {}
                    }
                }
                if owes {
                    for v in self.views.iter().filter(|v| v.spec.kind != Kind::Grouped) {
                        self.require_owed_while_held(
                            v.spec.provider as usize,
                            "the focused block changed under a flat view",
                        )?;
                    }
                }
            }
            Step::SetTemplate { view, template } => {
                if self.views.is_empty() {
                    return Ok(());
                }
                let i = *view as usize % self.views.len();
                let v = &mut self.views[i];
                v.view.set_template(template.expr());
                if v.spec.kind == Kind::Grouped {
                    return Ok(());
                }
                if v.spec.template.widget() != template.widget() {
                    v.unjudged.get_or_insert(RETEMPLATE_KEEPS_WIDGET);
                }
                v.spec.template = *template;
                if self.hold.is_none() {
                    v.applied_template = None;
                }
                if v.touched && *template != v.built_template {
                    v.unjudged.get_or_insert(RETEMPLATE_REVERTS);
                }
                let owes = v
                    .applied_template
                    .is_some_and(|applied| applied != *template);
                let p = v.spec.provider as usize;
                if owes {
                    self.require_owed_while_held(p, "a flat view's item template changed")?;
                }
            }
            Step::SetSpace { view, width } => {
                if self.views.is_empty() {
                    return Ok(());
                }
                let i = *view as usize % self.views.len();
                let v = &mut self.views[i];
                v.view.set_space(width.map(space));
                v.width = *width;
                if v.spec.kind == Kind::Grouped {
                    if v.spec.template == Template::Width {
                        v.unjudged.get_or_insert(GROUPED_KEEPS_SPACE);
                    }
                    return Ok(());
                }
                if self.hold.is_none() {
                    v.rebuilt_width = None;
                }
                v.touch();
                let owes = v.rebuilt_width.is_some_and(|rebuilt| rebuilt != *width);
                let p = v.spec.provider as usize;
                if owes {
                    self.require_owed_while_held(p, "a flat view's space changed")?;
                }
            }
            Step::TripInsideRow { row, to } => {
                let id = EntityUri::parse(&row_id(*row)).expect("a drawn row id parses");
                let armed = match *to {
                    Trip::Space { width, .. } if self.views.is_empty() => {
                        self.pending_space_trip = Some((id, width.map(space)));
                        return Ok(());
                    }
                    Trip::Space { view, width } => {
                        let v = &self.views[view as usize % self.views.len()];
                        let mutable = v.view.space_mutable().expect("a collection has a space");
                        ArmedTrip::Space(mutable.clone(), width.map(space))
                    }
                    Trip::Profile => ArmedTrip::Profile,
                    Trip::Focus { row } => ArmedTrip::Focus(
                        row.map(|r| EntityUri::parse(&row_id(r)).expect("a drawn row id parses")),
                    ),
                };
                *self.services.trip.lock().unwrap() = Some((id, armed));
            }
            Step::StartView(spec) => self.start_view(*spec),
            Step::DropView(view) => {
                if !self.views.is_empty() {
                    let i = *view as usize % self.views.len();
                    self.views.remove(i);
                }
            }
            Step::HoldWorkers => {
                if self.hold.is_none() {
                    self.hold = Some(Hold::all_workers(&self.rt, self.workers));
                }
            }
            Step::ReleaseWorkers => {
                if self.hold.take().is_some() {
                    self.settled("after the tokio workers were released")?;
                }
            }
            Step::Pause { interprets } => std::thread::sleep(
                self.services.interpret_delay * u32::from(*interprets)
                    + self.services.interpret_delay / 2,
            ),
        }
        Ok(())
    }

    fn apply_row_change(&mut self, provider: usize, change: RowChange) {
        let rows = &self.providers[provider];
        match change {
            RowChange::Upsert { row, content, lane } => {
                let data = ModelRow {
                    content: CONTENTS[content as usize],
                    lane: LANES[lane as usize],
                };
                let raw: DataRow = HashMap::from([
                    ("id".to_string(), Value::String(row_id(row))),
                    (
                        "content".to_string(),
                        Value::String(data.content.to_string()),
                    ),
                    ("lane".to_string(), Value::String(data.lane.to_string())),
                ]);
                let enriched = EnrichedRow::from_raw(raw, |_| HashMap::new());
                let change = match self.model[provider].insert(row, data) {
                    Some(_) => Change::Updated {
                        id: row_id(row),
                        data: enriched,
                        origin: remote(),
                    },
                    None => Change::Created {
                        data: enriched,
                        origin: remote(),
                    },
                };
                rows.apply_change(change, 1);
            }
            RowChange::Delete { row } => {
                self.model[provider].remove(&row);
                rows.apply_change(
                    Change::Deleted {
                        id: row_id(row),
                        origin: remote(),
                    },
                    1,
                );
            }
        }
    }

    /// With no input pending, `owed()` empties, and once it is empty every
    /// live view shows its provider's rows under the current profile and its
    /// current template.
    fn settled(&mut self, when: &str) -> Result<(), String> {
        let deadline = Instant::now() + QUIET_DEADLINE;
        loop {
            let owed = self.services.watermark.owed();
            if owed.is_empty() {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "{when}: owed() never emptied with no further input: {owed:?}"
                ));
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        if let Some(m) = self.mismatch() {
            return Err(format!("{when}: owed() is empty, but {m}"));
        }
        self.applied_focus = Some(self.focused_row());
        for v in &mut self.views {
            v.rebuilt_width = Some(v.width);
            v.applied_template = Some(v.spec.template);
            v.touched = false;
        }
        Ok(())
    }

    fn focused_row(&self) -> Option<u8> {
        let focus = self.services.focus.get_cloned()?;
        Some(
            (0..ROWS)
                .find(|r| EntityUri::parse(&row_id(*r)).expect("a drawn row id parses") == focus)
                .expect("the test focuses only drawn rows"),
        )
    }

    fn mismatch(&self) -> Option<String> {
        let generation = self
            .services
            .generation_of(&self.services.profiles.get_cloned());
        let focus = self.focused_row();
        self.views.iter().find_map(|v| {
            if self.oracle == Oracle::SkipKnownDefects && v.unjudged.is_some() {
                return None;
            }
            let mut expected: Vec<Vec<String>> = self.model[v.spec.provider as usize]
                .iter()
                .map(|(r, row)| {
                    v.spec
                        .template
                        .texts(row, generation, focus == Some(*r), v.width)
                })
                .collect();
            expected.sort();
            let mut shown: Vec<Vec<String>> = match v.spec.kind {
                Kind::Flat | Kind::Tree => {
                    v.view.items.lock_ref().iter().map(|i| texts(i)).collect()
                }
                Kind::Grouped => v
                    .view
                    .items
                    .lock_ref()
                    .iter()
                    .flat_map(|lane| lane.children.iter().map(|card| texts(card)))
                    .collect(),
            };
            shown.sort();
            (shown != expected).then(|| {
                format!(
                    "the {:?} view over {} ({:?}) shows {shown:?}; \
                     its rows under profile p{generation}, focus {focus:?} are {expected:?}",
                    v.spec.kind,
                    label(v.spec.provider as usize),
                    v.spec.template,
                )
            })
        })
    }
}

fn run_case(case: &Case, oracle: Oracle) -> Result<(), String> {
    let mut run = Run::new(case, oracle);
    for (i, step) in case.steps.iter().enumerate() {
        run.apply(step)
            .map_err(|e| format!("step {i} {step:?}: {e}"))?;
    }
    run.hold = None;
    run.settled("after the last step")?;
    let unrun = run.services.trip.lock().unwrap().take();
    match unrun {
        Some((row, _)) => Err(format!(
            "the case never interprets {row}, so its trip never ran"
        )),
        None => Ok(()),
    }
}

fn one_flat(template: Template) -> Vec<ViewSpec> {
    vec![ViewSpec {
        kind: Kind::Flat,
        provider: 0,
        template,
    }]
}

fn upsert(row: u8, content: u8, lane: u8) -> Step {
    Step::Batch {
        provider: 0,
        changes: vec![RowChange::Upsert { row, content, lane }],
        seq: Seq::Next,
    }
}

/// A flat view started over `rows` delivered rows, and `change` after
/// `interprets` interprets of its first pass.
fn first_pass(
    workers: usize,
    interpret_delay: Duration,
    rows: u8,
    template: Template,
    interprets: u8,
    change: Step,
) -> Case {
    let changes = (0..rows)
        .map(|row| RowChange::Upsert {
            row,
            content: row % CONTENTS.len() as u8,
            lane: 0,
        })
        .collect();
    Case {
        workers,
        interpret_delay,
        views: Vec::new(),
        steps: vec![
            Step::Batch {
                provider: 0,
                changes,
                seq: Seq::Next,
            },
            Step::StartView(ViewSpec {
                kind: Kind::Flat,
                provider: 0,
                template,
            }),
            Step::Pause { interprets },
            change,
        ],
    }
}

/// The interpret delay is long enough that the pause lands inside the first
/// pass on a loaded host too; later, the change only turns the case green.
fn first_pass_case() -> impl Strategy<Value = Case> {
    let change = prop_oneof![
        Just(Step::ProfileChange),
        prop_oneof![Just(320u16), Just(800), Just(1400)].prop_map(|width| Step::SetSpace {
            view: 0,
            width: Some(width)
        }),
    ];
    (
        1usize..=3,
        prop_oneof![Just(30u64), Just(60)],
        3u8..=ROWS,
        prop_oneof![Just(Template::Entity), Just(Template::Width)],
        1u8..=2,
        change,
    )
        .prop_map(|(workers, delay_ms, rows, template, interprets, change)| {
            first_pass(
                workers,
                Duration::from_millis(delay_ms),
                rows,
                template,
                interprets,
                change,
            )
        })
}

#[derive(Clone, Copy, Debug)]
enum RoundTrip {
    Space { from: Option<u16>, to: Option<u16> },
    Profile,
    Focus { from: Option<u8>, to: Option<u8> },
}

/// How long the input stays away before it returns.
#[derive(Clone, Copy, Debug)]
enum Gap {
    /// The test thread sleeps through `interprets` interprets and a half.
    Interprets(u8),
    /// It leaves and returns inside the interpret of row `row`.
    InsideRow(u8),
}

/// One view and one batch of `rows` rows, delivered before the view starts
/// (its first pass) or after (a later pass); `offset` interprets into that
/// pass the input goes from its value to another and returns after `gap`.
#[derive(Clone, Copy, Debug)]
struct RoundTripCase {
    kind: Kind,
    workers: usize,
    delay_ms: u64,
    rows: u8,
    rows_first: bool,
    input: RoundTrip,
    offset: u8,
    gap: Gap,
}

fn input_round_trip(c: RoundTripCase) -> Case {
    let changes = (0..c.rows)
        .map(|row| RowChange::Upsert {
            row,
            content: row % CONTENTS.len() as u8,
            lane: 0,
        })
        .collect();
    let batch = Step::Batch {
        provider: 0,
        changes,
        seq: Seq::Next,
    };
    let set = |width| Step::SetSpace { view: 0, width };
    let (template, start, there, back, trip) = match c.input {
        RoundTrip::Space { from, to } => (
            Template::Width,
            set(from),
            set(to),
            set(from),
            Trip::Space { view: 0, width: to },
        ),
        RoundTrip::Profile => (
            Template::Entity,
            Step::Pause { interprets: 0 },
            Step::ProfileChange,
            Step::ProfileRevert,
            Trip::Profile,
        ),
        RoundTrip::Focus { from, to } => {
            let focus = |row| Step::SetFocus { row };
            (
                Template::Entity,
                focus(from),
                focus(to),
                focus(from),
                Trip::Focus { row: to },
            )
        }
    };
    let view = Step::StartView(ViewSpec {
        kind: c.kind,
        provider: 0,
        template,
    });
    let steps = match c.gap {
        Gap::Interprets(gap) => {
            let mut steps = Vec::new();
            if c.rows_first {
                steps.push(batch.clone());
            }
            steps.extend([view, start, Step::Pause { interprets: 1 }]);
            if !c.rows_first {
                steps.push(batch);
            }
            steps.extend([
                Step::Pause {
                    interprets: c.offset,
                },
                there,
                Step::Pause { interprets: gap },
                back,
            ]);
            steps
        }
        Gap::InsideRow(row) => {
            let arm = Step::TripInsideRow { row, to: trip };
            match (c.rows_first, c.input) {
                // A view's space is set once the view exists, so its first
                // pass runs under no space and the trip leaves from there.
                (true, RoundTrip::Space { .. }) => vec![batch, arm, view, start],
                (true, _) => vec![batch, start, arm, view],
                (false, _) => vec![view, start, Step::Pause { interprets: 1 }, arm, batch],
            }
        }
    };
    Case {
        workers: c.workers,
        interpret_delay: Duration::from_millis(c.delay_ms),
        views: Vec::new(),
        steps,
    }
}

fn round_trip_input() -> impl Strategy<Value = RoundTrip> {
    let width = || prop::option::of(prop_oneof![Just(320u16), Just(800), Just(1400)]);
    let narrow = |w: Option<u16>| w.is_some_and(|w| w < NARROW_BELOW);
    // Rows 0..3 are in every drawn batch, so each focus value shows on a row.
    let focus_row = || prop::option::of(0u8..3);
    prop_oneof![
        (width(), width())
            .prop_filter("the two widths render differently", move |(a, b)| {
                narrow(*a) != narrow(*b)
            })
            .prop_map(|(from, to)| RoundTrip::Space { from, to }),
        Just(RoundTrip::Profile),
        (focus_row(), focus_row())
            .prop_filter("the focus moves", |(a, b)| a != b)
            .prop_map(|(from, to)| RoundTrip::Focus { from, to }),
    ]
}

fn round_trip_case(
    kind: Kind,
    input: impl Strategy<Value = RoundTrip>,
) -> impl Strategy<Value = Case> {
    (
        1usize..=3,
        prop_oneof![Just(0u64), Just(30), Just(60)],
        3u8..=ROWS,
        any::<bool>(),
        input,
        0u8..=1,
        // Rows 0..3 are in every drawn batch, so the trip's row is interpreted.
        prop_oneof![
            (0u8..=2).prop_map(Gap::Interprets),
            (0u8..3).prop_map(Gap::InsideRow),
        ],
    )
        .prop_map(
            move |(workers, delay_ms, rows, rows_first, input, offset, gap)| {
                input_round_trip(RoundTripCase {
                    kind,
                    workers,
                    delay_ms,
                    rows,
                    rows_first,
                    input,
                    offset,
                    gap,
                })
            },
        )
}

fn input_round_trip_case() -> impl Strategy<Value = Case> {
    round_trip_case(Kind::Flat, round_trip_input())
}

/// Focus only: the tree driver has no space or profile driver, so a change of
/// either is never re-rendered, round trip or not (a known base defect).
fn tree_round_trip_case() -> impl Strategy<Value = Case> {
    round_trip_case(
        Kind::Tree,
        round_trip_input().prop_filter("the tree driver re-renders focus only", |input| {
            matches!(input, RoundTrip::Focus { .. })
        }),
    )
}

/// Runs every case, so one red case does not hide the others.
fn run_found(found: &[Case]) {
    let failures: Vec<String> = found
        .iter()
        .enumerate()
        .filter_map(|(i, case)| {
            run_case(case, Oracle::Full)
                .err()
                .map(|e| format!("case {i}: {e}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn case_of(views: Vec<ViewSpec>, steps: Vec<Step>) -> Case {
    Case {
        workers: 1,
        interpret_delay: Duration::ZERO,
        views,
        steps,
    }
}

/// Cases the generator found, run through the same property.
#[test]
fn every_owed_driver_settles_on_found_cases() {
    let found = [
        case_of(
            one_flat(Template::Content),
            vec![
                Step::StartView(ViewSpec {
                    kind: Kind::Flat,
                    provider: 0,
                    template: Template::Entity,
                }),
                Step::HoldWorkers,
                upsert(0, 0, 0),
                Step::ReleaseWorkers,
                Step::ProfileChange,
            ],
        ),
        case_of(
            one_flat(Template::Content),
            vec![
                Step::HoldWorkers,
                Step::SetSpace {
                    view: 0,
                    width: Some(320),
                },
                Step::SetSpace {
                    view: 0,
                    width: None,
                },
            ],
        ),
        first_pass(
            1,
            Duration::from_millis(60),
            3,
            Template::Entity,
            1,
            Step::ProfileChange,
        ),
        first_pass(
            1,
            Duration::from_millis(60),
            3,
            Template::Width,
            1,
            Step::SetSpace {
                view: 0,
                width: Some(320),
            },
        ),
        input_round_trip(RoundTripCase {
            kind: Kind::Flat,
            workers: 1,
            delay_ms: 30,
            rows: 3,
            rows_first: false,
            input: RoundTrip::Space {
                from: Some(800),
                to: Some(320),
            },
            offset: 0,
            gap: Gap::Interprets(1),
        }),
        input_round_trip(RoundTripCase {
            kind: Kind::Flat,
            workers: 1,
            delay_ms: 30,
            rows: 3,
            rows_first: false,
            input: RoundTrip::Profile,
            offset: 0,
            gap: Gap::Interprets(1),
        }),
        input_round_trip(RoundTripCase {
            kind: Kind::Flat,
            workers: 1,
            delay_ms: 30,
            rows: 3,
            rows_first: true,
            input: RoundTrip::Profile,
            offset: 0,
            gap: Gap::Interprets(1),
        }),
        input_round_trip(RoundTripCase {
            kind: Kind::Flat,
            workers: 1,
            delay_ms: 30,
            rows: 3,
            rows_first: false,
            input: RoundTrip::Focus {
                from: Some(0),
                to: Some(1),
            },
            offset: 0,
            gap: Gap::Interprets(1),
        }),
        input_round_trip(RoundTripCase {
            kind: Kind::Flat,
            workers: 1,
            delay_ms: 30,
            rows: 4,
            rows_first: true,
            input: RoundTrip::Profile,
            offset: 0,
            gap: Gap::Interprets(1),
        }),
        input_round_trip(RoundTripCase {
            kind: Kind::Flat,
            workers: 2,
            delay_ms: 30,
            rows: 4,
            rows_first: true,
            input: RoundTrip::Focus {
                from: Some(0),
                to: Some(1),
            },
            offset: 0,
            gap: Gap::Interprets(1),
        }),
        input_round_trip(RoundTripCase {
            kind: Kind::Flat,
            workers: 1,
            delay_ms: 0,
            rows: 3,
            rows_first: false,
            input: RoundTrip::Focus {
                from: None,
                to: Some(0),
            },
            offset: 0,
            gap: Gap::InsideRow(0),
        }),
        input_round_trip(RoundTripCase {
            kind: Kind::Tree,
            workers: 1,
            delay_ms: 0,
            rows: 3,
            rows_first: false,
            input: RoundTrip::Focus {
                from: None,
                to: Some(0),
            },
            offset: 0,
            gap: Gap::InsideRow(0),
        }),
    ];
    run_found(&found);
}

#[test]
#[ignore = "set_template does not reach a flat driver's later rows and rebuilds or another widget, and the grouped driver keeps the profile and space it rendered with"]
fn every_live_view_shows_its_template_and_profile() {
    let set_template = |template| Step::SetTemplate { view: 0, template };
    let found = [
        case_of(
            one_flat(Template::Content),
            vec![upsert(0, 1, 0), set_template(Template::Entity)],
        ),
        case_of(
            one_flat(Template::Lane),
            vec![
                set_template(Template::Content),
                Step::Batch {
                    provider: 0,
                    changes: vec![
                        RowChange::Upsert {
                            row: 2,
                            content: 0,
                            lane: 2,
                        },
                        RowChange::Upsert {
                            row: 0,
                            content: 1,
                            lane: 1,
                        },
                        RowChange::Upsert {
                            row: 1,
                            content: 2,
                            lane: 1,
                        },
                    ],
                    seq: Seq::Next,
                },
            ],
        ),
        case_of(
            vec![ViewSpec {
                kind: Kind::Grouped,
                provider: 0,
                template: Template::Entity,
            }],
            vec![upsert(0, 0, 0), Step::HoldWorkers, Step::ProfileChange],
        ),
        case_of(
            vec![ViewSpec {
                kind: Kind::Grouped,
                provider: 0,
                template: Template::Width,
            }],
            vec![
                upsert(0, 0, 0),
                Step::HoldWorkers,
                Step::ReleaseWorkers,
                Step::SetSpace {
                    view: 0,
                    width: Some(320),
                },
            ],
        ),
    ];
    run_found(&found);
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: None,
        // A stuck driver costs a quiet deadline per shrink step; report within the test timeout.
        max_shrink_time: 60_000,
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_owed_driver_settles_and_an_empty_watermark_means_the_rows_are_shown(case in case()) {
        run_case(&case, Oracle::SkipKnownDefects).map_err(TestCaseError::fail)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 16,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn an_input_change_inside_a_first_interpret_pass_is_owed_until_shown(case in first_pass_case()) {
        run_case(&case, Oracle::Full).map_err(TestCaseError::fail)?;
    }

    #[test]
    fn an_input_that_returns_to_its_value_inside_an_interpret_pass_is_owed_until_shown(case in input_round_trip_case()) {
        run_case(&case, Oracle::Full).map_err(TestCaseError::fail)?;
    }

    #[test]
    fn an_input_that_returns_to_its_value_under_a_tree_view_is_owed_until_shown(case in tree_round_trip_case()) {
        run_case(&case, Oracle::Full).map_err(TestCaseError::fail)?;
    }
}

/// A tree view whose focus moves between settled frames: each move must
/// re-render both the row that lost focus and the row that gained it.
fn tree_focus_moves_case() -> impl Strategy<Value = Case> {
    (
        1usize..=3,
        3u8..=ROWS,
        prop::collection::vec(prop::option::of(0u8..3), 2..=4),
    )
        .prop_map(|(workers, rows, moves)| {
            let mut steps: Vec<Step> = (0..rows)
                .map(|r| upsert(r, r % CONTENTS.len() as u8, 0))
                .collect();
            for row in moves {
                steps.extend([
                    Step::HoldWorkers,
                    Step::ReleaseWorkers,
                    Step::SetFocus { row },
                ]);
            }
            Case {
                workers,
                interpret_delay: Duration::ZERO,
                views: vec![ViewSpec {
                    kind: Kind::Tree,
                    provider: 0,
                    template: Template::Entity,
                }],
                steps,
            }
        })
}

#[test]
fn a_tree_view_shows_each_settled_focus_move_on_found_cases() {
    let mut steps: Vec<Step> = (0..3).map(|r| upsert(r, r, 0)).collect();
    for row in [Some(0), Some(1)] {
        steps.extend([
            Step::HoldWorkers,
            Step::ReleaseWorkers,
            Step::SetFocus { row },
        ]);
    }
    run_found(&[Case {
        workers: 1,
        interpret_delay: Duration::ZERO,
        views: vec![ViewSpec {
            kind: Kind::Tree,
            provider: 0,
            template: Template::Entity,
        }],
        steps,
    }]);
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_tree_view_shows_each_settled_focus_move(case in tree_focus_moves_case()) {
        run_case(&case, Oracle::Full).map_err(TestCaseError::fail)?;
    }
}
