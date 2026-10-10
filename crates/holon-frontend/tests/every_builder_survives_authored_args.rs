//! Every registered widget builder and value function, called from an authored
//! render source with arbitrary arguments and then fed changing rows, returns a
//! ViewModel (an `error` node is fine) and never panics, in the build or in a
//! task it spawned: a panic aborts the app (`panic = "abort"`). A collection's
//! layout keywords and `rules:` are judged once, the same way on first paint
//! and on a view-mode click.
//!
//! `PROPTEST_CASES` sets the cases per builder (default 256). A deep run
//! (thousands) exceeds nextest's 2-minute cap; run it with `cargo test`.
//!
//! Own test binary: the panic hook is process-global.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view::start_reactive_views;
use holon_frontend::reactive_view_model::ItemFlow;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::shadow_builders::all_widget_metas;
use holon_frontend::shadow_builders::build_shadow_interpreter;
use holon_frontend::value_fns::synthetic::SyntheticRows;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestError;
use proptest::test_runner::TestRunner;

const BUILDER_SOURCES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/shadow_builders");
const VALUE_FN_SOURCES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/value_fns");
/// The helpers every builder's args pass through.
const SHARED_BUILDER_HELPERS: &[&str] = &[
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/render_interpreter.rs"),
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../holon-api/src/render_eval.rs"
    ),
];
/// Keys a builder reads under a name its source spells only as a prefix.
const DYNAMIC_KEYS: &[(&str, &[&str])] = &[("view_mode_switcher", &["mode_tree", "mode_table"])];

/// Named-arg keys a source reads: `get_<kind>("key"` and `.get("key"`.
/// Raw builders declare no params, so their source is the only listing.
fn arg_names_read_by(source: &str) -> BTreeSet<String> {
    let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    let mut names = BTreeSet::new();
    for (at, _) in compact.match_indices("(\"") {
        let head = &compact[..at];
        let accessor = head
            .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .next()
            .expect("rsplit yields at least one piece");
        let method = accessor.rsplit('.').next().expect("non-empty");
        if !(method.starts_with("get_") || method == "get") {
            continue;
        }
        let key: String = compact[at + 2..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !key.is_empty() && compact[at + 2 + key.len()..].starts_with('"') {
            names.insert(key);
        }
    }
    names
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// Per builder: its declared params plus every key its own source file reads;
/// `shared` holds the keys the shared helpers read.
fn arg_names_per_builder() -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let shared: BTreeSet<String> = SHARED_BUILDER_HELPERS
        .iter()
        .flat_map(|path| arg_names_read_by(&read(path)))
        .collect();
    let mut all_sources = String::new();
    for entry in std::fs::read_dir(BUILDER_SOURCES).expect("builder source dir") {
        all_sources.push_str(&read(
            entry.expect("dir entry").path().to_str().expect("utf-8"),
        ));
    }
    let any_builder = arg_names_read_by(&all_sources);

    let interpreter = build_shadow_interpreter();
    let mut widgets: Vec<String> = interpreter.supported_widgets().into_iter().collect();
    widgets.sort();
    assert!(
        widgets.len() > 30,
        "the shadow interpreter registers suspiciously few builders: {widgets:?}"
    );
    let metas = all_widget_metas();
    let mut per_builder: Vec<(String, Vec<String>)> = widgets
        .into_iter()
        .map(|name| {
            let mut own: BTreeSet<String> = metas
                .iter()
                .filter(|m| m.name == name)
                .flat_map(|m| m.params.iter().map(|p| p.name.to_string()))
                .collect();
            let file = format!("{BUILDER_SOURCES}/{name}.rs");
            if std::path::Path::new(&file).exists() {
                own.extend(arg_names_read_by(&read(&file)));
            }
            if own.is_empty() {
                own.extend(any_builder.iter().cloned());
            }
            for (builder, keys) in DYNAMIC_KEYS {
                if *builder == name {
                    own.extend(keys.iter().map(|k| k.to_string()));
                }
            }
            (name, own.into_iter().collect())
        })
        .collect();
    let mut value_fns: Vec<String> = interpreter.supported_value_fns().into_iter().collect();
    value_fns.sort();
    assert!(
        value_fns.len() >= 4,
        "the shadow interpreter registers suspiciously few value functions: {value_fns:?}"
    );
    per_builder.extend(value_fns.into_iter().map(|name| {
        let file = format!("{VALUE_FN_SOURCES}/{name}.rs");
        let own = match std::path::Path::new(&file).exists() {
            true => arg_names_read_by(&read(&file)).into_iter().collect(),
            false => Vec::new(),
        };
        (format!("{VALUE_FN_PREFIX}{name}"), own)
    }));
    (per_builder, shared.into_iter().collect())
}

/// Marks a value function in the builder list: it is called as the
/// `collection:` of a `list`, where an authored source calls it.
const VALUE_FN_PREFIX: &str = "value_fn:";

/// Render-DSL values of every shape an author can write where a builder
/// expects something else.
const AUTHORED_VALUES: &[&str] = &[
    r#""""#,
    r#""middle""#,
    r#""zz""#,
    r#""start""#,
    r#""wrap""#,
    r#""block:p""#,
    r#""[\"tree\"]""#,
    "0",
    "-3",
    "1.5",
    "true",
    "[]",
    r#"["a"]"#,
    "[1, 2]",
    "#{}",
    r#"col("content")"#,
    r#"col("absent")"#,
    r#"text("x")"#,
    r#"list(#{item_template: text("x")})"#,
    r#"columns(#{gap: "zz", item_template: text("x")})"#,
    r#"tree(#{horizontal: true, wrap: "wrap", item_template: text("x")})"#,
    r#"["TODO", "DONE"]"#,
    r#"[1, "DONE"]"#,
    r#"[#{when: always(), override: #{show_bullet: false}}]"#,
    r#"[#{when: 5, override: #{}}]"#,
];

/// Each of [`AUTHORED_VALUES`] as the parser hands it to a builder, parsed
/// once: parsing dominates the cost of a case.
fn parsed_values() -> &'static [RenderExpr] {
    static PARSED: OnceLock<Vec<RenderExpr>> = OnceLock::new();
    PARSED.get_or_init(|| {
        AUTHORED_VALUES
            .iter()
            .map(|v| {
                let call = holon_api::render_dsl::parse_render_dsl(&format!("text(#{{v: {v}}})"))
                    .unwrap_or_else(|e| panic!("`{v}` is not a render-DSL value: {e:#}"));
                let RenderExpr::FunctionCall { mut args, .. } = call else {
                    panic!("`text(...)` parses to a call");
                };
                assert_eq!(args.len(), 1, "`{v}` parses to exactly one named arg");
                args.remove(0).value
            })
            .collect()
    })
}

fn authored_value() -> impl Strategy<Value = RenderExpr> {
    proptest::sample::select(parsed_values().to_vec())
}

fn row_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        Just(Value::String(String::new())),
        "[a-z]{1,6}".prop_map(Value::String),
        Just(Value::Integer(-1)),
        Just(Value::Boolean(true)),
    ]
}

#[derive(Clone)]
struct Call {
    expr: RenderExpr,
    row: DataRow,
    /// The rows the built node sees next, through its live row handle.
    changes: Vec<DataRow>,
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` over row {:?}, then {:?}",
            self.expr.to_rhai(),
            self.row,
            self.changes
        )
    }
}

/// Built the way the parser builds a call: a map argument's fields become
/// named args, sorted by key, ahead of the positional ones.
fn call_of(builder: String, own: Vec<String>, shared: Vec<String>) -> impl Strategy<Value = Call> {
    let own_args = proptest::collection::vec(proptest::option::of(authored_value()), own.len());
    let shared_args =
        proptest::collection::vec((proptest::sample::select(shared), authored_value()), 0..3);
    let positional = proptest::collection::vec(authored_value(), 0..3);
    let row = proptest::option::of((row_value(), row_value()));
    let changes = proptest::collection::vec(proptest::option::of((row_value(), row_value())), 1..3);
    (own_args, shared_args, positional, row, changes).prop_map(
        move |(own_args, shared_args, positional, row, changes)| {
            let mut named: BTreeMap<String, RenderExpr> = own
                .iter()
                .zip(own_args)
                .filter_map(|(k, v)| v.map(|v| (k.clone(), v)))
                .collect();
            for (k, v) in shared_args {
                named.entry(k).or_insert(v);
            }
            let mut positional_args = Vec::new();
            for value in positional {
                match value {
                    RenderExpr::Object { fields } => named.extend(fields),
                    other => positional_args.push(Arg {
                        name: None,
                        value: other,
                    }),
                }
            }
            let args = named
                .into_iter()
                .map(|(k, value)| Arg {
                    name: Some(k),
                    value,
                })
                .chain(positional_args)
                .collect();
            let expr = match builder.strip_prefix(VALUE_FN_PREFIX) {
                None => RenderExpr::FunctionCall {
                    name: builder.clone(),
                    args,
                },
                Some(value_fn) => RenderExpr::FunctionCall {
                    name: "list".to_string(),
                    args: vec![
                        Arg {
                            name: Some("collection".to_string()),
                            value: RenderExpr::FunctionCall {
                                name: value_fn.to_string(),
                                args,
                            },
                        },
                        Arg {
                            name: Some("item_template".to_string()),
                            value: RenderExpr::ColumnRef {
                                name: "content".to_string(),
                            },
                        },
                    ],
                },
            };
            Call {
                expr,
                row: row_of(row),
                changes: changes.into_iter().map(row_of).collect(),
            }
        },
    )
}

fn row_of(columns: Option<(Value, Value)>) -> DataRow {
    columns
        .map(|(content, other)| {
            DataRow::from([
                ("id".to_string(), Value::String("block:p".to_string())),
                ("content".to_string(), content),
                ("other".to_string(), other),
            ])
        })
        .unwrap_or_default()
}

thread_local! {
    static CASE_PANICS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

static SPAWNED_PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Logs a panic inside a case for that case; any other panic is reported as
/// usual and also kept, since it may come from a task a builder spawned.
fn record_case_panic(info: &std::panic::PanicHookInfo<'_>) -> bool {
    CASE_PANICS.with(|log| match log.borrow_mut().as_mut() {
        Some(log) => {
            log.push(info.to_string());
            true
        }
        None => false,
    })
}

/// Builds `call` against a live row handle, keeps the node alive with its
/// reactive views started, and walks the row through `call.changes`.
fn build(services: &Arc<dyn BuilderServices>, call: &Call) -> Result<(), TestCaseError> {
    let row = Mutable::new(Arc::new(call.row.clone()));
    let ctx = RenderContext::default().with_row_mutable(row.read_only());
    let rt = services.runtime_handle();
    CASE_PANICS.with(|log| *log.borrow_mut() = Some(Vec::new()));
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let vm = services.interpret(&call.expr, &ctx);
        start_reactive_views(&vm, services, &rt);
        serde_json::to_value(vm.snapshot()).expect("a ViewModel serializes");
        for change in &call.changes {
            row.set(Arc::new(change.clone()));
            // One worker thread: a task queued behind the row's wake-ups runs
            // after them.
            rt.block_on(async { rt.spawn(async {}).await.expect("the drain task runs") });
        }
        serde_json::to_value(vm.snapshot()).expect("a ViewModel serializes");
    }));
    let panics = CASE_PANICS
        .with(|log| log.borrow_mut().take())
        .expect("the case log was installed above");
    if built.is_ok() && panics.is_empty() {
        return Ok(());
    }
    Err(TestCaseError::fail(format!(
        "{call:?} panicked: {}",
        panics.join(" | ")
    )))
}

/// The minimal panicking call for `builder`, or `None`.
fn panicking_call(
    services: &Arc<dyn BuilderServices>,
    builder: &str,
    own: Vec<String>,
    shared: Vec<String>,
) -> Option<String> {
    let mut runner = TestRunner::new(Config {
        failure_persistence: None,
        ..Config::default()
    });
    let outcome = runner.run(&call_of(builder.to_string(), own, shared), |call| {
        build(services, &call)
    });
    match outcome {
        Ok(()) => None,
        Err(TestError::Fail(reason, call)) => {
            Some(format!("{builder}: minimal {call:?}\n    {reason}"))
        }
        Err(TestError::Abort(reason)) => panic!("{builder}: runner aborted: {reason}"),
    }
}

#[test]
fn every_builder_returns_a_view_model_for_any_authored_args() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !record_case_panic(info) {
            SPAWNED_PANICS
                .lock()
                .expect("panic log")
                .push(info.to_string());
            report(info);
        }
    }));
    let services: Arc<dyn BuilderServices> = Arc::new(StubBuilderServices::new());
    let (builders, shared) = arg_names_per_builder();
    let workers = std::thread::available_parallelism()
        .expect("the host reports its parallelism")
        .get();
    let queue = Mutex::new(builders);
    let failures = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some((builder, own)) = queue.lock().expect("queue").pop() {
                    if let Some(failure) = panicking_call(&services, &builder, own, shared.clone())
                    {
                        failures.lock().expect("failures").push(failure);
                    }
                }
            });
        }
    });
    let mut failures = failures.into_inner().expect("failures");
    failures.sort();
    let spawned = std::mem::take(&mut *SPAWNED_PANICS.lock().expect("panic log"));
    assert!(
        failures.is_empty() && spawned.is_empty(),
        "{} builder(s) or value function(s) panic on authored args:\n{}\npanics in tasks the builders spawned: {spawned:#?}",
        failures.len(),
        failures.join("\n")
    );
}

// ── One answer per authored layout keyword ─────────────────────────────────

/// A registered collection layout and the call that builds it.
#[derive(Clone, Copy, Debug)]
struct CollectionBuilder {
    builder: &'static str,
    layout: &'static str,
    /// Args the call needs to reach `layout`.
    args: &'static str,
    /// The gap it lays out at when its call site names none.
    default_gap: f32,
    /// Whether it can lay its items side by side (`horizontal:` / `wrap:`).
    flows: bool,
}

const fn collection(
    builder: &'static str,
    layout: &'static str,
    args: &'static str,
    default_gap: f32,
    flows: bool,
) -> CollectionBuilder {
    CollectionBuilder {
        builder,
        layout,
        args,
        default_gap,
        flows,
    }
}

/// Every registered collection layout.
const COLLECTION_BUILDERS: &[CollectionBuilder] = &[
    collection("list", "list", "", 4.0, true),
    collection("columns", "columns", "", 16.0, false),
    collection("tree", "tree", "", 2.0, false),
    collection("outline", "outline", "", 2.0, false),
    collection("table", "table", "", 2.0, false),
    collection(
        "table",
        "table_columnar",
        r#"columns: [#{header: "c", cell: text("x")}]"#,
        4.0,
        false,
    ),
    collection("board", "board", "", 16.0, false),
];

/// A layout keyword's authored value: a literal, or a column of the row.
#[derive(Clone, Copy, Debug)]
enum Kw {
    Lit(&'static str),
    Col(&'static str),
}

const GAPS: &[Kw] = &[
    Kw::Lit("0"),
    Kw::Lit("7"),
    Kw::Lit("1.5"),
    Kw::Lit(r#""zz""#),
    Kw::Lit("true"),
    Kw::Col("n"),
];
const HORIZONTALS: &[Kw] = &[
    Kw::Lit("true"),
    Kw::Lit("false"),
    Kw::Lit(r#""true""#),
    Kw::Lit("1"),
    Kw::Col("b"),
];
const WRAPS: &[Kw] = &[
    Kw::Lit(r#""wrap""#),
    Kw::Lit(r#""nowrap""#),
    Kw::Lit(r#""maybe""#),
    Kw::Lit("true"),
    Kw::Col("w"),
];

fn literal(source: &str) -> Value {
    let RenderExpr::FunctionCall { args, .. } =
        holon_api::render_dsl::parse_render_dsl(&format!("text(#{{v: {source}}})"))
            .expect("a layout keyword value parses")
    else {
        panic!("`text(...)` parses to a call");
    };
    match &args[0].value {
        RenderExpr::Literal { value } => value.clone(),
        other => panic!("`{source}` is not a literal: {other:?}"),
    }
}

impl Kw {
    fn source(self) -> String {
        match self {
            Kw::Lit(s) => s.to_string(),
            Kw::Col(c) => format!("col(\"{c}\")"),
        }
    }

    fn resolve(self, row: &DataRow) -> Value {
        match self {
            Kw::Lit(s) => literal(s),
            Kw::Col(c) => row[c].clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct LayoutCase {
    collection: CollectionBuilder,
    gap: Option<Kw>,
    horizontal: Option<Kw>,
    wrap: Option<Kw>,
    row: DataRow,
}

impl LayoutCase {
    fn source(&self) -> String {
        let mut args = vec![r#"item_template: text("x")"#.to_string()];
        if !self.collection.args.is_empty() {
            args.push(self.collection.args.to_string());
        }
        for (name, kw) in [
            ("gap", self.gap),
            ("horizontal", self.horizontal),
            ("wrap", self.wrap),
        ] {
            if let Some(kw) = kw {
                args.push(format!("{name}: {}", kw.source()));
            }
        }
        format!("{}(#{{{}}})", self.collection.builder, args.join(", "))
    }

    fn expr(&self) -> RenderExpr {
        holon_frontend::shadow_builders::register_render_dsl_widget_names();
        holon_api::render_dsl::parse_render_dsl(&self.source())
            .unwrap_or_else(|e| panic!("`{}` parses: {e:#}", self.source()))
    }

    /// The layout the source names, or `None` when a keyword is refused.
    fn model(&self) -> Option<(f32, ItemFlow)> {
        let resolve = |kw: Option<Kw>| kw.map(|kw| kw.resolve(&self.row));
        let gap = match resolve(self.gap) {
            None => self.collection.default_gap,
            Some(Value::Integer(i)) => i as f32,
            Some(Value::Float(f)) => f as f32,
            Some(_) => return None,
        };
        let horizontal = match resolve(self.horizontal) {
            None => false,
            Some(Value::Boolean(b)) => b,
            Some(_) => return None,
        };
        if !self.collection.flows && (self.horizontal.is_some() || self.wrap.is_some()) {
            return None;
        }
        let flow = match (horizontal, resolve(self.wrap)) {
            (false, None) => ItemFlow::Stacked,
            (true, None) => ItemFlow::Row,
            (true, Some(Value::String(s))) if s == "wrap" => ItemFlow::WrappingRow,
            (true, Some(Value::String(s))) if s == "nowrap" => ItemFlow::Row,
            _ => return None,
        };
        Some((gap, flow))
    }
}

fn layout_case() -> impl Strategy<Value = LayoutCase> {
    let row = (
        proptest::sample::select(vec![
            Value::Integer(5),
            Value::Float(2.5),
            Value::String("x".to_string()),
            Value::Boolean(false),
        ]),
        proptest::sample::select(vec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::String("yes".to_string()),
        ]),
        proptest::sample::select(vec![
            Value::String("wrap".to_string()),
            Value::String("nowrap".to_string()),
            Value::Integer(1),
        ]),
    );
    (
        proptest::sample::select(COLLECTION_BUILDERS),
        proptest::option::of(proptest::sample::select(GAPS)),
        proptest::option::of(proptest::sample::select(HORIZONTALS)),
        proptest::option::of(proptest::sample::select(WRAPS)),
        row,
    )
        .prop_map(
            |(collection, gap, horizontal, wrap, (n, b, w))| LayoutCase {
                collection,
                gap,
                horizontal,
                wrap,
                row: DataRow::from([
                    ("id".to_string(), Value::String("block:p".to_string())),
                    ("n".to_string(), n),
                    ("b".to_string(), b),
                    ("w".to_string(), w),
                ]),
            },
        )
}

/// What a leg made of a source: the layout it draws, or the message of the
/// error node it draws instead.
#[derive(Debug, PartialEq)]
enum Answer {
    Layout {
        name: String,
        gap: f32,
        flow: ItemFlow,
    },
    Refused(String),
}

fn answer_of(node: &ReactiveViewModel) -> Answer {
    if node.is_error() {
        return Answer::Refused(
            node.prop_str("message")
                .expect("an error node carries a message"),
        );
    }
    let layout = node
        .collection
        .as_ref()
        .and_then(|view| view.layout())
        .unwrap_or_else(|| {
            panic!(
                "a collection builder draws a collection: {:?}",
                node.widget_name()
            )
        });
    Answer::Layout {
        name: layout.name().to_string(),
        gap: layout.gap,
        flow: layout.flow,
    }
}

/// The build leg's answer against the model: a refused keyword draws an error
/// node naming the builder, an accepted one reaches the layout.
fn judge(case: &LayoutCase, answer: &Answer) -> Result<(), TestCaseError> {
    match (case.model(), answer) {
        (None, Answer::Refused(message)) => {
            prop_assert!(
                message.contains(case.collection.builder),
                "`{}` over {:?}: the error node does not name `{}`: {message:?}",
                case.source(),
                case.row,
                case.collection.builder
            );
        }
        (None, Answer::Layout { .. }) => {
            prop_assert!(
                false,
                "`{}` over {:?} names a layout keyword the builder must refuse, and it drew {answer:?}",
                case.source(),
                case.row
            );
        }
        (Some((gap, flow)), answer) => {
            prop_assert_eq!(
                answer,
                &Answer::Layout {
                    name: case.collection.layout.to_string(),
                    gap,
                    flow
                },
                "`{}` over {:?}",
                case.source(),
                case.row
            );
        }
    }
    Ok(())
}

/// The context a collection is painted in: a live data source, as the main
/// panel has, and the case's row.
fn layout_ctx(case: &LayoutCase) -> RenderContext {
    RenderContext {
        data_source: Some(Arc::new(SyntheticRows::from_rows(Vec::new()))),
        ..RenderContext::default()
    }
    .with_row(Arc::new(case.row.clone()))
}

fn build_leg(services: &Arc<dyn BuilderServices>, case: &LayoutCase) -> Answer {
    answer_of(&services.interpret(&case.expr(), &layout_ctx(case)))
}

/// A `view_mode_switcher` offering mode `a` (a plain list) and mode `b` (the
/// case's source), first painted in `default_mode`.
fn switcher(
    services: &Arc<dyn BuilderServices>,
    case: &LayoutCase,
    default_mode: &str,
) -> ReactiveViewModel {
    let source = format!(
        r#"view_mode_switcher(#{{entity_uri: "block:p", modes: "[{{\"name\":\"a\",\"icon\":\"list\"}},{{\"name\":\"b\",\"icon\":\"list\"}}]", default_mode: "{default_mode}", mode_a: list(#{{item_template: text("x")}}), mode_b: {}}})"#,
        case.source()
    );
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl(&source)
        .unwrap_or_else(|e| panic!("`{source}` parses: {e:#}"));
    services.interpret(&expr, &layout_ctx(case))
}

/// The slot's answer, after checking the bar marks `mode` active exactly when
/// the slot draws it.
fn slot_answer(switcher: &ReactiveViewModel, mode: &str) -> Result<Answer, TestCaseError> {
    let slot = switcher
        .slot
        .as_ref()
        .expect("a view_mode_switcher has a slot")
        .content
        .get_cloned();
    let answer = answer_of(&slot);
    let active = switcher.prop_str("active_mode");
    match &answer {
        Answer::Refused(_) => {
            prop_assert_eq!(active, None, "a mode is marked active over an error node")
        }
        Answer::Layout { .. } => prop_assert_eq!(active.as_deref(), Some(mode)),
    }
    Ok(answer)
}

/// The first paint of a switcher whose default mode is the case's source.
fn first_paint_leg(
    services: &Arc<dyn BuilderServices>,
    case: &LayoutCase,
) -> Result<Answer, TestCaseError> {
    slot_answer(&switcher(services, case, "b"), "b")
}

/// A click from mode `a` to the case's source.
fn click_leg(
    services: &Arc<dyn BuilderServices>,
    case: &LayoutCase,
) -> Result<Answer, TestCaseError> {
    let vm = switcher(services, case, "a");
    click(&vm, "b", services);
    slot_answer(&vm, "b")
}

fn run_property<S: Strategy>(strategy: S, test: impl Fn(S::Value) -> Result<(), TestCaseError>) {
    let mut runner = TestRunner::new(Config {
        failure_persistence: None,
        ..Config::default()
    });
    if let Err(e) = runner.run(&strategy, test) {
        panic!("{e}");
    }
}

fn layout_services() -> Arc<dyn BuilderServices> {
    Arc::new(StubBuilderServices::new())
}

#[test]
fn every_collection_builder_honours_or_refuses_its_layout_keywords() {
    let services = layout_services();
    run_property(layout_case(), |case| {
        judge(&case, &build_leg(&services, &case))
    });
}

#[test]
fn the_view_mode_click_and_the_first_paint_give_one_answer_per_source() {
    let services = layout_services();
    run_property(layout_case(), |case| {
        let built = build_leg(&services, &case);
        prop_assert_eq!(
            &first_paint_leg(&services, &case)?,
            &built,
            "`{}` over {:?}: a switcher's first paint vs the bare build",
            case.source(),
            case.row
        );
        prop_assert_eq!(
            &click_leg(&services, &case)?,
            &built,
            "`{}` over {:?}: a view-mode click vs the first paint",
            case.source(),
            case.row
        );
        Ok(())
    });
}

// ── `rules:` is parsed, never dropped ──────────────────────────────────────

const RULE_ENTRIES: &[(&str, bool)] = &[
    (
        r#"#{when: always(), override: #{show_bullet: false}}"#,
        true,
    ),
    (r#"#{when: eq("level", 0), override: #{}}"#, true),
    (r#"#{when: 5, override: #{}}"#, false),
    (r#"#{override: #{}}"#, false),
    ("3", false),
    (r#""x""#, false),
];

#[test]
fn a_malformed_rules_entry_draws_an_error_node_naming_the_builder() {
    let services = StubBuilderServices::new();
    let builders: Vec<&'static str> = COLLECTION_BUILDERS
        .iter()
        .map(|c| c.builder)
        .chain(["row", "section"])
        .collect();
    let rules = prop_oneof![
        proptest::collection::vec(proptest::sample::select(RULE_ENTRIES), 0..3).prop_map(
            |entries| {
                let valid = entries.iter().all(|(_, ok)| *ok);
                let source = entries
                    .iter()
                    .map(|(s, _)| *s)
                    .collect::<Vec<_>>()
                    .join(", ");
                (format!("[{source}]"), valid)
            }
        ),
        Just((r#""x""#.to_string(), false)),
        Just(("#{}".to_string(), false)),
    ];
    run_property(
        (proptest::sample::select(builders), rules),
        |(builder, (rules, valid))| {
            holon_frontend::shadow_builders::register_render_dsl_widget_names();
            let source = format!(r#"{builder}(#{{item_template: text("x"), rules: {rules}}})"#);
            let expr = holon_api::render_dsl::parse_render_dsl(&source)
                .unwrap_or_else(|e| panic!("`{source}` parses: {e:#}"));
            let node = services.interpret(&expr, &RenderContext::default());
            match valid {
                true => prop_assert!(
                    !node.is_error(),
                    "`{source}` draws {:?}",
                    node.prop_str("message")
                ),
                false => {
                    prop_assert!(
                        node.is_error(),
                        "`{source}` drops its malformed rules in silence"
                    );
                    let message = node
                        .prop_str("message")
                        .expect("an error node carries a message");
                    prop_assert!(
                        message.contains(builder),
                        "`{source}`: the error node does not name `{builder}`: {message:?}"
                    );
                }
            }
            Ok(())
        },
    );
}

// ── A typed param's authored value is honoured or refused ───────────────────

/// Values an author can give a typed param: literals of every shape and
/// columns of [`typed_param_row`].
const PARAM_VALUES: &[&str] = &[
    r#""zz""#,
    r#""start""#,
    "0",
    "7",
    "1.5",
    "true",
    "[]",
    r#"["a"]"#,
    "#{}",
    r#"col("n")"#,
    r#"col("s")"#,
    r#"col("b")"#,
    r#"col("absent")"#,
];

fn typed_param_row() -> DataRow {
    DataRow::from([
        ("id".to_string(), Value::String("block:p".to_string())),
        ("n".to_string(), Value::Integer(5)),
        ("s".to_string(), Value::String("x".to_string())),
        ("b".to_string(), Value::Boolean(true)),
    ])
}

/// What a positional arg of a builder binds to, seen from one typed param.
#[derive(Clone, Copy, Debug)]
enum Positional {
    /// The param reads this positional slot.
    Slot(usize),
    /// The builder has a Collection param: every positional arg is a child.
    Child,
    /// The param is named-only (a Bool).
    Named,
}

/// A builder's typed param, with what a positional arg binds to.
#[derive(Clone, Debug)]
struct TypedParam {
    builder: &'static str,
    name: &'static str,
    type_hint: &'static str,
    positional: Positional,
}

/// Every String / Number / Bool param a `widget_builder!` builder declares.
/// String and Number params consume positional slots in declaration order,
/// except in a builder with a Collection param, whose positional args are all
/// children.
fn typed_params() -> Vec<TypedParam> {
    let mut params = Vec::new();
    for meta in all_widget_metas() {
        let mut slot = 0;
        let has_children = meta.params.iter().any(|p| p.type_hint == "Collection");
        for p in meta.params {
            let consumes_slot =
                !has_children && matches!(p.type_hint, "String" | "Number" | "Value");
            if matches!(p.type_hint, "String" | "Number" | "Bool") {
                params.push(TypedParam {
                    builder: meta.name,
                    name: p.name,
                    type_hint: p.type_hint,
                    positional: match (has_children, consumes_slot) {
                        (true, _) => Positional::Child,
                        (false, true) => Positional::Slot(slot),
                        (false, false) => Positional::Named,
                    },
                });
            }
            if consumes_slot {
                slot += 1;
            }
        }
    }
    assert!(
        params
            .iter()
            .any(|p| matches!(p.positional, Positional::Child)),
        "no typed param of a builder with children: {params:?}"
    );
    assert!(
        params.len() > 20,
        "suspiciously few typed params: {params:?}"
    );
    params
}

/// The prop a builder draws for an accepted `value` of a `type_hint` param, or
/// the refusal it owes; `None` for a value that counts as absent.
fn typed_param_model(type_hint: &str, value: &Value) -> Option<Result<Value, ()>> {
    match (type_hint, value) {
        (_, Value::Null) => None,
        ("Number", Value::Integer(i)) => Some(Ok(Value::Float(*i as f64))),
        ("Number", Value::Float(f)) => Some(Ok(Value::Float(*f))),
        ("Bool", Value::Boolean(b)) => Some(Ok(Value::Boolean(*b))),
        ("String", Value::String(s)) => Some(Ok(Value::String(s.clone()))),
        ("String", Value::Integer(i)) => Some(Ok(Value::String(i.to_string()))),
        ("String", Value::Float(f)) => Some(Ok(Value::String(f.to_string()))),
        ("String", Value::Boolean(b)) => Some(Ok(Value::String(b.to_string()))),
        _ => Some(Err(())),
    }
}

/// Props a builder derives from its param rather than copying it.
const DERIVED_PROPS: &[(&str, &str)] =
    &[("text", "color"), ("icon", "color"), ("badge", "block_id")];

#[derive(Clone, Debug)]
struct TypedParamCase {
    param: TypedParam,
    value: &'static str,
    positional: bool,
}

impl TypedParamCase {
    fn source(&self) -> String {
        match (self.positional, self.param.positional) {
            (true, Positional::Slot(slot)) => {
                let mut args = vec![r#"col("absent")"#; slot];
                args.push(self.value);
                format!("{}({})", self.param.builder, args.join(", "))
            }
            (true, Positional::Child) => format!("{}({})", self.param.builder, self.value),
            _ => format!(
                "{}(#{{{}: {}}})",
                self.param.builder, self.param.name, self.value
            ),
        }
    }

    /// The value the call hands this param, as the interpreter resolves it.
    fn resolved(&self, expr: &RenderExpr) -> Value {
        let RenderExpr::FunctionCall { args, .. } = expr else {
            panic!("`{}` parses to a call", self.source());
        };
        let arg = match (self.positional, self.param.positional) {
            (true, Positional::Slot(slot)) => args.iter().filter(|a| a.name.is_none()).nth(slot),
            (true, Positional::Child) => None,
            _ => args
                .iter()
                .find(|a| a.name.as_deref() == Some(self.param.name)),
        };
        arg.map_or(Value::Null, |arg| {
            let services = StubBuilderServices::new();
            let row = typed_param_row();
            let ctx = RenderContext::default().with_row(Arc::new(row.clone()));
            let fns = services.value_fn_lookup(&ctx);
            let value = holon_api::render_eval::eval_to_interp(
                &arg.value,
                &holon_api::render_eval::EvalEnv::of_row(&row),
                &*fns,
            );
            match value {
                Ok(holon_api::InterpValue::Value(v)) => v,
                Ok(holon_api::InterpValue::Rows(_)) => panic!("`{}` is a row set", self.source()),
                Err(e) => panic!("`{}` evaluates: {e}", self.source()),
            }
        })
    }
}

fn typed_param_case() -> impl Strategy<Value = TypedParamCase> {
    let params = typed_params();
    let child_params: Vec<TypedParam> = params
        .iter()
        .filter(|p| matches!(p.positional, Positional::Child))
        .cloned()
        .collect();
    (
        prop_oneof![
            3 => proptest::sample::select(params),
            1 => proptest::sample::select(child_params),
        ],
        proptest::sample::select(PARAM_VALUES),
        any::<bool>(),
    )
        .prop_map(|(param, value, positional)| TypedParamCase {
            param,
            value,
            positional,
        })
}

#[test]
fn every_typed_param_is_honoured_or_refused() {
    let services: Arc<dyn BuilderServices> = Arc::new(StubBuilderServices::new());
    run_property(typed_param_case(), |case| {
        holon_frontend::shadow_builders::register_render_dsl_widget_names();
        let source = case.source();
        let builder = case.param.builder;
        // The parser refuses some values itself (a colour that names no theme
        // token): a refusal at the boundary, before any builder runs.
        let expr = match holon_api::render_dsl::parse_render_dsl(&source) {
            Ok(expr) => expr,
            Err(e) => {
                let message = format!("{e:#}");
                prop_assert!(
                    message.contains(builder),
                    "`{source}`: the parse error does not name `{builder}`: {message}"
                );
                return Ok(());
            }
        };
        let row = Arc::new(typed_param_row());
        let ctx = RenderContext::default().with_row(row.clone());
        let node = services.interpret(&expr, &ctx);
        let key = case.param.name;
        if case.positional && matches!(case.param.positional, Positional::Child) {
            let bare = services.interpret(
                &holon_api::render_dsl::parse_render_dsl(&format!("{builder}()"))
                    .expect("a bare call parses"),
                &ctx,
            );
            prop_assert!(
                !node.is_error(),
                "`{source}`: a positional arg of a builder with children is a child, but the \
                 builder drew an error node: {:?}",
                node.prop_str("message")
            );
            prop_assert_eq!(
                node.props.get_cloned().get(key).cloned(),
                bare.props.get_cloned().get(key).cloned(),
                "`{}`: a positional arg must not bind `{}`",
                source,
                key
            );
            let RenderExpr::FunctionCall { args, .. } = &expr else {
                panic!("`{source}` parses to a call");
            };
            prop_assert_eq!(
                node.snapshot().children().len(),
                args.iter().filter(|a| a.name.is_none()).count(),
                "`{}`: every positional arg is drawn as a child",
                source
            );
            return Ok(());
        }
        let message = node.prop_str("message").filter(|_| node.is_error());
        let fast = holon_frontend::render_interpreter::is_props_only_widget(builder).then(|| {
            holon_frontend::render_interpreter::resolve_props(
                builder, &expr, &row, &*services, None,
            )
        });
        match typed_param_model(case.param.type_hint, &case.resolved(&expr)) {
            None => {}
            Some(Err(())) => {
                let message = message.unwrap_or_else(|| {
                    format!(
                        "no error node: drew {:?} with props {:?}",
                        node.widget_name(),
                        node.props.get_cloned()
                    )
                });
                prop_assert!(
                    message.contains(builder) && message.contains(key),
                    "`{source}` must be refused by an error node naming `{builder}` and `{key}`: {message}"
                );
                prop_assert!(
                    !matches!(fast, Some(Ok(_))),
                    "`{source}`: the props fast path accepts what the build refuses: {fast:?}"
                );
            }
            // The `error` builder draws its accepted message as an error node.
            Some(Ok(expected)) => match message.filter(|_| builder != "error") {
                Some(message) => prop_assert!(
                    message.contains(builder),
                    "`{source}`: the error node does not name `{builder}`: {message}"
                ),
                None => {
                    let props = node.props.get_cloned();
                    if let Some(drawn) = props
                        .get(key)
                        .filter(|_| !DERIVED_PROPS.contains(&(builder, key)))
                    {
                        prop_assert_eq!(drawn, &expected, "`{}` draws `{}`", source, key);
                    }
                    if let Some(fast) = fast {
                        let fast = fast.map_err(|_| {
                            TestCaseError::fail(format!(
                                "`{source}`: the props fast path refuses what the build accepts"
                            ))
                        })?;
                        prop_assert_eq!(
                            fast.get(key),
                            props.get(key),
                            "`{}`: fast path vs build",
                            source
                        );
                    }
                }
            },
        }
        Ok(())
    });
}

// ── A view_mode_switcher draws the mode it marks active ────────────────────

/// Each mode a switcher can offer, with the layout its template draws.
const SWITCHER_MODES: &[(&str, &str, &str)] = &[
    ("a", "list", r#"list(#{item_template: text("x")})"#),
    ("b", "tree", r#"tree(#{item_template: text("x")})"#),
    ("c", "table", r#"table(#{item_template: text("x")})"#),
];

#[derive(Clone, Debug)]
struct SwitcherCase {
    /// The modes `modes:` lists, in order.
    listed: Vec<&'static str>,
    /// The modes given a `mode_<name>` template.
    templated: Vec<&'static str>,
    default_mode: Option<&'static str>,
    click: &'static str,
}

impl SwitcherCase {
    fn source(&self) -> String {
        let modes: Vec<String> = self
            .listed
            .iter()
            .map(|m| format!(r#"{{\"name\":\"{m}\",\"icon\":\"list\"}}"#))
            .collect();
        let mut args = vec![
            r#"entity_uri: "block:p""#.to_string(),
            format!(r#"modes: "[{}]""#, modes.join(",")),
        ];
        if let Some(default_mode) = self.default_mode {
            args.push(format!("default_mode: {default_mode}"));
        }
        for (mode, _, template) in SWITCHER_MODES {
            if self.templated.contains(mode) {
                args.push(format!("mode_{mode}: {template}"));
            }
        }
        format!("view_mode_switcher(#{{{}}})", args.join(", "))
    }

    /// The mode the first paint draws, or `None` when the source is refused.
    fn first_mode(&self) -> Option<&'static str> {
        let mut listed = self.listed.clone();
        let mut templated = self.templated.clone();
        listed.sort();
        templated.sort();
        if listed.is_empty() || listed != templated {
            return None;
        }
        match self.default_mode {
            None => Some(self.listed[0]),
            Some(lit) => self
                .listed
                .iter()
                .copied()
                .find(|m| lit == format!("\"{m}\"")),
        }
    }
}

fn layout_of(mode: &str) -> &'static str {
    SWITCHER_MODES
        .iter()
        .find(|(m, _, _)| *m == mode)
        .map(|(_, layout, _)| *layout)
        .expect("a switcher mode")
}

fn switcher_case() -> impl Strategy<Value = SwitcherCase> {
    let modes = || proptest::sample::subsequence(vec!["a", "b", "c"], 0..=3);
    (
        modes(),
        prop_oneof![3 => Just(None), 1 => modes().prop_map(Some)],
        proptest::option::of(proptest::sample::select(vec![
            r#""a""#, r#""b""#, r#""c""#, "5",
        ])),
        proptest::sample::select(vec!["a", "b", "c"]),
    )
        .prop_map(|(listed, templated, default_mode, click)| SwitcherCase {
            templated: templated.unwrap_or_else(|| listed.clone()),
            listed,
            default_mode,
            click,
        })
}

/// A click on `mode`'s button, as GPUI's bar sends it.
fn click(vm: &ReactiveViewModel, mode: &str, services: &Arc<dyn BuilderServices>) {
    vm.view_mode_switch().switch_mode(mode, services);
}

/// The slot draws the layout of the mode the bar marks active, or an error
/// node with no mode active.
fn drawn_mode(vm: &ReactiveViewModel) -> Result<Option<String>, TestCaseError> {
    let slot = vm
        .slot
        .as_ref()
        .expect("a view_mode_switcher has a slot")
        .content
        .get_cloned();
    let active = vm.prop_str("active_mode");
    match answer_of(&slot) {
        Answer::Refused(message) => {
            prop_assert!(
                message.contains("view_mode_switcher"),
                "the slot's error node does not name view_mode_switcher: {message}"
            );
            prop_assert_eq!(&active, &None, "a mode is marked active over an error node");
            Ok(None)
        }
        Answer::Layout { name, .. } => {
            let active = active.expect("a drawn mode is marked active");
            prop_assert_eq!(
                name,
                layout_of(&active),
                "the slot draws another mode than the active `{}`",
                active
            );
            Ok(Some(active))
        }
    }
}

#[test]
fn a_view_mode_switcher_draws_the_mode_it_marks_active() {
    let services = layout_services();
    run_property(switcher_case(), |case| {
        holon_frontend::shadow_builders::register_render_dsl_widget_names();
        let source = case.source();
        let expr = holon_api::render_dsl::parse_render_dsl(&source)
            .unwrap_or_else(|e| panic!("`{source}` parses: {e:#}"));
        let ctx = RenderContext {
            data_source: Some(Arc::new(SyntheticRows::from_rows(Vec::new()))),
            ..RenderContext::default()
        }
        .with_row(Arc::new(typed_param_row()));
        let vm = services.interpret(&expr, &ctx);
        let Some(first) = case.first_mode() else {
            let message = vm.prop_str("message").filter(|_| vm.is_error());
            prop_assert!(
                message
                    .as_deref()
                    .is_some_and(|m| m.contains("view_mode_switcher")),
                "`{source}` must be refused by an error node naming view_mode_switcher, drew {:?} active {:?}",
                vm.widget_name(),
                vm.prop_str("active_mode")
            );
            return Ok(());
        };
        let painted = drawn_mode(&vm)?;
        prop_assert_eq!(painted.as_deref(), Some(first), "`{}`: first paint", source);
        click(&vm, case.click, &services);
        let expected = case.listed.contains(&case.click).then_some(case.click);
        let clicked = drawn_mode(&vm)?;
        prop_assert_eq!(
            clicked.as_deref(),
            expected,
            "`{}`: a click on `{}`",
            source,
            case.click
        );
        Ok(())
    });
}
