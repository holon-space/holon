//! Token measurement: org (full) vs org (ID-compressed) vs TOON, on real vault
//! files, for both the full block set and the "exclude DONE" projection.
//!
//! Run: `cargo run -p holon-toon --example measure --features measure --
//! <file.org> [more.org ...]`
//!
//! Tokenizer: `tiktoken-rs` `o200k_base` (the GPT-4o/o200k BPE). Claude uses a
//! different tokenizer, but o200k is a well-correlated public proxy for
//! English + code and is fully reproducible; treat the absolute counts as
//! representative and the *ratios* as the finding.

use std::collections::BTreeMap;

use holon_toon::Forest;
use holon_toon::Table;
use holon_toon::ToonValue;
use holon_toon::org_reader;
use holon_toon::render;
use tiktoken_rs::o200k_base;

struct Row {
    label: String,
    org_full: usize,
    org_comp: usize,
    toon: usize,
    blocks: usize,
    // tokens consumed by the bare block ids alone — the irreducible cost BOTH
    // formats must pay once per block.
    id_tokens: usize,
    // round-trip check for the TOON rendering
    toon_roundtrips: bool,
}

fn id_tokens(bpe: &tiktoken_rs::CoreBPE, forest: &Forest) -> usize {
    forest
        .flatten()
        .iter()
        .map(|(_, b)| count(bpe, b.id.as_str()))
        .sum()
}

fn count(bpe: &tiktoken_rs::CoreBPE, s: &str) -> usize {
    bpe.encode_with_special_tokens(s).len()
}

fn measure(bpe: &tiktoken_rs::CoreBPE, label: &str, forest: &Forest) -> Row {
    let org_full = org_reader::render_org_full(forest);
    let org_comp = org_reader::render_org_compressed(forest);
    let toon = render(forest);

    let roundtrips = match holon_toon::parse(&toon) {
        Ok(parsed) => &parsed == forest,
        Err(_) => false,
    };

    Row {
        label: label.to_string(),
        org_full: count(bpe, &org_full),
        org_comp: count(bpe, &org_comp),
        toon: count(bpe, &toon),
        blocks: forest.block_count(),
        id_tokens: id_tokens(bpe, forest),
        toon_roundtrips: roundtrips,
    }
}

fn pct(base: usize, other: usize) -> f64 {
    if base == 0 {
        return 0.0;
    }
    100.0 * (base as f64 - other as f64) / base as f64
}

// ---------------------------------------------------------------------------
// Generic tabular: JSON vs TOON on synthetic query-result shapes.
//
// All data here is SYNTHETIC — no vault content. We measure the ROWS PAYLOAD
// both ways (a JSON array of row objects vs one TOON tabular array), the
// apples-to-apples comparison of what an MCP query tool would emit.
// ---------------------------------------------------------------------------

fn tv_str(s: &str) -> ToonValue {
    ToonValue::Str(s.to_string())
}

/// One synthetic dataset: a name and its rows (already typed).
fn synthetic_shapes() -> Vec<(&'static str, Vec<BTreeMap<String, ToonValue>>)> {
    let mut shapes: Vec<(&'static str, Vec<BTreeMap<String, ToonValue>>)> = Vec::new();

    // 1. Wide uniform table: 40 rows × 8 columns, every column present.
    let states = ["TODO", "DOING", "DONE", "WAIT"];
    let wide: Vec<_> = (0..40)
        .map(|i| {
            BTreeMap::from([
                ("id".to_string(), ToonValue::Int(1000 + i)),
                ("state".to_string(), tv_str(states[(i % 4) as usize])),
                (
                    "priority".to_string(),
                    tv_str(["A", "B", "C"][(i % 3) as usize]),
                ),
                ("effort".to_string(), ToonValue::Int((i % 8) * 15)),
                ("assignee".to_string(), tv_str(&format!("agent-{}", i % 5))),
                ("done".to_string(), ToonValue::Bool(i % 4 == 2)),
                (
                    "progress".to_string(),
                    ToonValue::Float((i as f64 % 10.0) / 10.0),
                ),
                (
                    "title".to_string(),
                    tv_str(&format!("Synthetic work item number {i}")),
                ),
            ])
        })
        .collect();
    shapes.push(("wide uniform (40×8)", wide));

    // 2. Narrow table: 60 rows × 2 columns.
    let narrow: Vec<_> = (0..60)
        .map(|i| {
            BTreeMap::from([
                ("id".to_string(), ToonValue::Int(i)),
                ("label".to_string(), tv_str(&format!("row label {i}"))),
            ])
        })
        .collect();
    shapes.push(("narrow (60×2)", narrow));

    // 3. Table with a nested JSON column (properties drawer as JSON string).
    let with_json: Vec<_> = (0..20)
        .map(|i| {
            let props = format!(
                "{{\"assigned-to\":\"agent-{}\",\"tags\":[\"x\",\"y\"],\"effort\":{}}}",
                i % 3,
                i * 5
            );
            BTreeMap::from([
                ("id".to_string(), ToonValue::Int(i)),
                ("state".to_string(), tv_str(states[(i % 4) as usize])),
                ("properties".to_string(), tv_str(&props)),
            ])
        })
        .collect();
    shapes.push(("with JSON column (20×3)", with_json));

    // 4. Tiny result: 2 rows × 3 columns.
    let tiny = vec![
        BTreeMap::from([
            ("id".to_string(), ToonValue::Int(1)),
            ("state".to_string(), tv_str("DONE")),
            ("title".to_string(), tv_str("The only finished thing")),
        ]),
        BTreeMap::from([
            ("id".to_string(), ToonValue::Int(2)),
            ("state".to_string(), tv_str("TODO")),
            ("title".to_string(), tv_str("The next thing")),
        ]),
    ];
    shapes.push(("tiny (2×3)", tiny));

    shapes
}

/// JSON rendering of a row set matching the MCP rows payload: an array of
/// objects. Type mirrors `holon_to_json_value` (Int/Float/Bool/Str/Null),
/// including parsing a JSON-string column back to real nested JSON so the JSON
/// baseline is not unfairly penalised by escaped quotes.
fn rows_to_json(rows: &[BTreeMap<String, ToonValue>]) -> String {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (k, v) in row {
                obj.insert(k.clone(), toon_to_json(v));
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    serde_json::to_string(&serde_json::Value::Array(arr)).unwrap()
}

fn toon_to_json(v: &ToonValue) -> serde_json::Value {
    match v {
        ToonValue::Str(s) => {
            // A cell that is itself valid JSON (the nested-column case) is
            // emitted as real JSON in the JSON baseline — the fair comparison.
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                if parsed.is_object() || parsed.is_array() {
                    return parsed;
                }
            }
            serde_json::Value::String(s.clone())
        }
        ToonValue::Int(i) => serde_json::Value::Number((*i).into()),
        ToonValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ToonValue::Bool(b) => serde_json::Value::Bool(*b),
        ToonValue::Null => serde_json::Value::Null,
    }
}

fn run_synthetic(bpe: &tiktoken_rs::CoreBPE) {
    println!("\n## Generic tabular — JSON vs TOON (synthetic query results)\n");
    println!(
        "{:<26} {:>5} {:>7} {:>7} {:>8} {:>5}",
        "shape", "rows", "json", "toon", "saved", "rt"
    );
    println!("{}", "-".repeat(64));
    for (label, rows) in synthetic_shapes() {
        let json = rows_to_json(&rows);
        let table = Table::from_rows("rows", rows.clone()).expect("valid table");
        let toon = table.render().expect("render");
        let roundtrips = Table::parse(&toon).map(|t| t == table).unwrap_or(false);
        let j = count(bpe, &json);
        let t = count(bpe, &toon);
        println!(
            "{:<26} {:>5} {:>7} {:>7} {:>7.1}% {:>5}",
            label,
            rows.len(),
            j,
            t,
            pct(j, t),
            if roundtrips { "ok" } else { "FAIL" },
        );
    }
    println!(
        "\nsaved = % tokens TOON saves vs the JSON rows payload (array of objects).\n\
         rt = TOON parse(render)==table. Tokenizer: tiktoken o200k_base."
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bpe = o200k_base().expect("load o200k_base tokenizer");

    // `measure synthetic` runs the JSON-vs-TOON generic-tabular comparison with
    // no vault files (fully reproducible, synthetic data only).
    if args.first().map(String::as_str) == Some("synthetic") {
        run_synthetic(&bpe);
        return;
    }

    if args.is_empty() {
        eprintln!(
            "usage:\n  measure <file.org> [more.org ...]   # org vs TOON on real files\n  \
             measure synthetic                    # JSON vs TOON on synthetic query results"
        );
        std::process::exit(2);
    }

    let mut rows: Vec<Row> = Vec::new();
    for path in &args {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
        let forest =
            org_reader::parse_org(&src).unwrap_or_else(|e| panic!("parse {}: {}", path, e));
        let name = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());

        rows.push(measure(&bpe, &format!("{} (all)", name), &forest));
        let filtered = org_reader::filter_exclude_done(&forest);
        rows.push(measure(&bpe, &format!("{} (no DONE)", name), &filtered));
    }

    println!(
        "{:<34} {:>7} {:>9} {:>9} {:>7} {:>8} {:>8} {:>8} {:>5}",
        "dataset", "blocks", "org_full", "org_comp", "toon", "vs_full", "vs_comp", "ids", "rt"
    );
    println!("{}", "-".repeat(104));
    for r in &rows {
        println!(
            "{:<34} {:>7} {:>9} {:>9} {:>7} {:>7.1}% {:>7.1}% {:>7} {:>5}",
            r.label,
            r.blocks,
            r.org_full,
            r.org_comp,
            r.toon,
            pct(r.org_full, r.toon),
            pct(r.org_comp, r.toon),
            r.id_tokens,
            if r.toon_roundtrips { "ok" } else { "FAIL" },
        );
    }
    println!(
        "\nvs_full = % tokens saved by TOON vs canonical org; \
         vs_comp = vs ID-compressed org.\n\
         ids = tokens spent on bare block ids alone (irreducible; paid once per \
         block by EVERY format).\n\
         rt = TOON parse(render)==forest. Tokenizer: tiktoken o200k_base."
    );
}
