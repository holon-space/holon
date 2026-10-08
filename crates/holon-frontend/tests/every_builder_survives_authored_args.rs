//! Every registered widget builder, called from an authored render source with
//! arbitrary arguments, returns a ViewModel (an `error` node is fine). A panic
//! in a builder aborts the app (`panic = "abort"`).
//!
//! Own test binary: the panic hook is process-global.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::shadow_builders::all_widget_metas;
use holon_frontend::shadow_builders::build_shadow_interpreter;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestError;
use proptest::test_runner::TestRunner;

const BUILDER_SOURCES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/shadow_builders");
const SHARED_BUILDER_HELPERS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/render_interpreter.rs");

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
    let shared = arg_names_read_by(&read(SHARED_BUILDER_HELPERS));
    let mut all_sources = String::new();
    for entry in std::fs::read_dir(BUILDER_SOURCES).expect("builder source dir") {
        all_sources.push_str(&read(
            entry.expect("dir entry").path().to_str().expect("utf-8"),
        ));
    }
    let any_builder = arg_names_read_by(&all_sources);

    let mut widgets: Vec<String> = build_shadow_interpreter()
        .supported_widgets()
        .into_iter()
        .collect();
    widgets.sort();
    assert!(
        widgets.len() > 30,
        "the shadow interpreter registers suspiciously few builders: {widgets:?}"
    );
    let metas = all_widget_metas();
    let per_builder = widgets
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
            (name, own.into_iter().collect())
        })
        .collect();
    (per_builder, shared.into_iter().collect())
}

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
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` over row {:?}", self.expr.to_rhai(), self.row)
    }
}

/// Built the way the parser builds a call: a map argument's fields become
/// named args, sorted by key, ahead of the positional ones.
fn call_of(builder: String, own: Vec<String>, shared: Vec<String>) -> impl Strategy<Value = Call> {
    let own_args = proptest::collection::vec(proptest::option::of(authored_value()), own.len());
    let shared_arg =
        proptest::option::weighted(0.2, (proptest::sample::select(shared), authored_value()));
    let positional = proptest::collection::vec(authored_value(), 0..3);
    let row = proptest::option::of((row_value(), row_value()));
    (own_args, shared_arg, positional, row).prop_map(
        move |(own_args, shared_arg, positional, row)| {
            let mut named: BTreeMap<String, RenderExpr> = own
                .iter()
                .zip(own_args)
                .filter_map(|(k, v)| v.map(|v| (k.clone(), v)))
                .collect();
            if let Some((k, v)) = shared_arg {
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
            let row = row
                .map(|(content, other)| {
                    DataRow::from([
                        ("id".to_string(), Value::String("block:p".to_string())),
                        ("content".to_string(), content),
                        ("other".to_string(), other),
                    ])
                })
                .unwrap_or_default();
            Call {
                expr: RenderExpr::FunctionCall {
                    name: builder.clone(),
                    args,
                },
                row,
            }
        },
    )
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

fn build(services: &StubBuilderServices, call: &Call) -> Result<(), TestCaseError> {
    let ctx = RenderContext::default().with_row(Arc::new(call.row.clone()));
    CASE_PANICS.with(|log| *log.borrow_mut() = Some(Vec::new()));
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let vm = services.interpret(&call.expr, &ctx).snapshot();
        serde_json::to_value(&vm).expect("a ViewModel serializes");
    }));
    let panics = CASE_PANICS
        .with(|log| log.borrow_mut().take())
        .expect("the case log was installed above");
    if built.is_ok() && panics.is_empty() {
        return Ok(());
    }
    // A source the DSL refuses never reaches a builder; the render path draws
    // its parse error instead.
    if holon_api::render_dsl::parse_render_dsl(&call.expr.to_rhai()).is_err() {
        return Ok(());
    }
    Err(TestCaseError::fail(format!(
        "{call:?} panicked: {}",
        panics.join(" | ")
    )))
}

/// The minimal panicking call for `builder`, or `None`.
fn panicking_call(
    services: &StubBuilderServices,
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
    let services = StubBuilderServices::new();
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
        "{} builder(s) panic on authored args:\n{}\npanics in tasks the builders spawned: {spawned:#?}",
        failures.len(),
        failures.join("\n")
    );
}
