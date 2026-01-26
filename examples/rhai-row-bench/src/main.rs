use rhai::{Dynamic, Engine, Scope, AST};
use std::collections::HashMap;
use std::time::Instant;

/// Mirrors holon_api::Value — we inline it to avoid pulling the whole crate
#[derive(Clone, Debug)]
enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Null,
}

fn build_rows(n: usize) -> Vec<HashMap<String, Value>> {
    (0..n)
        .map(|i| {
            let mut row = HashMap::new();
            row.insert("id".into(), Value::String(format!("block-{i}")));
            row.insert(
                "content".into(),
                Value::String(format!("Some content for block {i}")),
            );
            row.insert(
                "content_type".into(),
                Value::String(if i % 3 == 0 {
                    "source".into()
                } else {
                    "text".into()
                }),
            );
            // ~40% of rows are tasks
            row.insert(
                "task_state".into(),
                if i % 5 < 2 {
                    Value::String(["TODO", "DONE", "IN_PROGRESS", "CANCELLED"][i % 4].into())
                } else {
                    Value::Null
                },
            );
            row.insert("priority".into(), Value::Integer((i % 4) as i64));
            row.insert("entity_name".into(), Value::String("blocks".into()));
            row
        })
        .collect()
}

fn value_to_dynamic(v: &Value) -> Dynamic {
    match v {
        Value::String(s) => Dynamic::from(s.clone()),
        Value::Integer(i) => Dynamic::from(*i),
        Value::Float(f) => Dynamic::from(*f),
        Value::Boolean(b) => Dynamic::from(*b),
        Value::Null => Dynamic::UNIT,
    }
}

fn row_to_scope(row: &HashMap<String, Value>, scope: &mut Scope) {
    scope.clear();
    for (k, v) in row {
        scope.push_dynamic(k.as_str(), value_to_dynamic(v));
    }
}

fn row_to_dynamic_map(row: &HashMap<String, Value>) -> Dynamic {
    let map: rhai::Map = row
        .iter()
        .map(|(k, v)| (k.clone().into(), value_to_dynamic(v)))
        .collect();
    Dynamic::from_map(map)
}

/// The expression we want to benchmark. Realistic complexity:
/// read entity_name, check task_state, branch on content_type and priority.
const RHAI_EXPR_SCOPE: &str = r#"
    if entity_name == "blocks" && task_state != () {
        if priority >= 2 { "task_high_priority" } else { "task_row" }
    } else if entity_name == "blocks" && content_type == "source" {
        "source_row"
    } else {
        "block_row"
    }
"#;

const RHAI_EXPR_MAP: &str = r#"
    let r = row;
    if r.entity_name == "blocks" && r.task_state != () {
        if r.priority >= 2 { "task_high_priority" } else { "task_row" }
    } else if r.entity_name == "blocks" && r.content_type == "source" {
        "source_row"
    } else {
        "block_row"
    }
"#;

/// Native Rust equivalent for comparison baseline
fn native_resolve(row: &HashMap<String, Value>) -> &'static str {
    let entity = match row.get("entity_name") {
        Some(Value::String(s)) => s.as_str(),
        _ => "",
    };
    let task_state_present = !matches!(row.get("task_state"), Some(Value::Null) | None);
    let priority = match row.get("priority") {
        Some(Value::Integer(i)) => *i,
        _ => 0,
    };
    let content_type = match row.get("content_type") {
        Some(Value::String(s)) => s.as_str(),
        _ => "",
    };

    if entity == "blocks" && task_state_present {
        if priority >= 2 {
            "task_high_priority"
        } else {
            "task_row"
        }
    } else if entity == "blocks" && content_type == "source" {
        "source_row"
    } else {
        "block_row"
    }
}

fn bench_scope(engine: &Engine, ast: &AST, rows: &[HashMap<String, Value>]) -> (usize, f64) {
    let mut scope = Scope::new();
    let start = Instant::now();
    let mut count = 0usize;
    for row in rows {
        row_to_scope(row, &mut scope);
        let _result: String = engine.eval_ast_with_scope(&mut scope, ast).unwrap();
        count += 1;
    }
    (count, start.elapsed().as_secs_f64())
}

fn bench_map(engine: &Engine, ast: &AST, rows: &[HashMap<String, Value>]) -> (usize, f64) {
    let mut scope = Scope::new();
    let start = Instant::now();
    let mut count = 0usize;
    for row in rows {
        scope.clear();
        scope.push("row", row_to_dynamic_map(row));
        let _result: String = engine.eval_ast_with_scope(&mut scope, ast).unwrap();
        count += 1;
    }
    (count, start.elapsed().as_secs_f64())
}

fn bench_native(rows: &[HashMap<String, Value>]) -> (usize, f64) {
    let start = Instant::now();
    let mut count = 0usize;
    for row in rows {
        let _result = native_resolve(row);
        count += 1;
    }
    (count, start.elapsed().as_secs_f64())
}

fn main() {
    let engine = Engine::new();
    let ast_scope = engine.compile(RHAI_EXPR_SCOPE).unwrap();
    let ast_map = engine.compile(RHAI_EXPR_MAP).unwrap();

    // Warm up
    let warmup = build_rows(100);
    bench_scope(&engine, &ast_scope, &warmup);
    bench_map(&engine, &ast_map, &warmup);
    bench_native(&warmup);

    println!("Rhai per-row evaluation benchmark (pre-compiled AST, release mode)");
    println!("Expression: check entity_name + task_state + content_type + priority → template ID");
    println!("{:-<90}", "");
    println!(
        "{:<12} {:>12} {:>12} {:>12} {:>15} {:>15}",
        "Rows", "Scope (ms)", "Map (ms)", "Native (ms)", "Scope µs/row", "Map µs/row"
    );
    println!("{:-<90}", "");

    for n in [1_000, 10_000, 100_000] {
        let rows = build_rows(n);

        let (_, t_scope) = bench_scope(&engine, &ast_scope, &rows);
        let (_, t_map) = bench_map(&engine, &ast_map, &rows);
        let (_, t_native) = bench_native(&rows);

        println!(
            "{:<12} {:>12.2} {:>12.2} {:>12.4} {:>15.2} {:>15.2}",
            n,
            t_scope * 1000.0,
            t_map * 1000.0,
            t_native * 1000.0,
            (t_scope / n as f64) * 1_000_000.0,
            (t_map / n as f64) * 1_000_000.0,
        );
    }

    println!("{:-<90}", "");
    println!("Note: 'Scope' = variables pushed directly into Rhai scope");
    println!("      'Map'   = row passed as a single Rhai Map object");
    println!("      'Native' = equivalent Rust match logic (baseline)");
}
