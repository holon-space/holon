//! Round-trip PBT for the **generic tabular codec** (`holon_toon::table`):
//! `Table::parse(table.render()) == table` over arbitrary generated row sets.
//!
//! The generator deliberately stresses every hazard the codec must survive:
//! - **awkward column keys** (commas, colons, quotes, braces, spaces, newlines,
//!   unicode, the empty string, and strings that look like `true`/`42`),
//! - **values across every `ToonValue` variant**, including strings that would
//!   be mistaken for bare literals (`"null"`, `"42"`, `"1.2.3"`, `""`),
//! - **heterogeneous rows** — each row independently keeps or drops each
//!   column, exercising the absent-vs-empty distinction on every cell,
//! - **empty tables** and **all-empty-row (zero-column) tables**.
//!
//! See `RED_LOG.md` for the red-for-the-right-reason seed (the absent-vs-empty
//! confusion) this property was driven against before the codec was trusted.

use holon_toon::Table;
use holon_toon::ToonValue;
use proptest::collection::btree_map;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;

/// Characters that stress the header + cell quoting/escaping paths.
const AWKWARD: &[char] = &[
    'a', 'Z', '0', '9', ' ', '\t', '\n', ':', ',', '"', '\\', '[', ']', '{', '}', '=', '-', '#',
    '.', 'e', 'é', '界',
];

fn awkward_string(min: usize, max: usize) -> impl Strategy<Value = String> {
    vec(select(AWKWARD), min..max).prop_map(|cs| cs.into_iter().collect())
}

/// A column key: awkward content, plus a few literal-lookalikes that MUST be
/// carried verbatim as header fields (`""`, `null`, `42`).
fn column_key() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => awkward_string(0, 6),
        1 => select(vec!["", "null", "true", "42", "1.5", "-"]).prop_map(String::from),
    ]
}

/// A value spanning every variant, weighted toward the string cases that alias
/// bare literals (the round-trip-critical ones).
fn value() -> impl Strategy<Value = ToonValue> {
    prop_oneof![
        6 => prop_oneof![
            awkward_string(0, 10).prop_map(ToonValue::Str),
            select(vec!["", "null", "true", "false", "42", "-3", "1.5", "1.2.3", "1e", "1-2", "  "])
                .prop_map(|s| ToonValue::Str(s.to_string())),
        ],
        3 => any::<i64>().prop_map(ToonValue::Int),
        3 => any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(ToonValue::Float),
        2 => any::<bool>().prop_map(ToonValue::Bool),
        1 => Just(ToonValue::Null),
    ]
}

/// A row over a FIXED column universe, where each column is independently
/// present-or-absent (so absent-vs-empty is exercised per cell). A
/// `Vec<Strategy<Value = Option<ToonValue>>>` is itself a strategy yielding
/// `Vec<Option<ToonValue>>`, one slot per column, which we zip back to keys.
fn row(cols: Vec<String>) -> impl Strategy<Value = std::collections::BTreeMap<String, ToonValue>> {
    let slot_strategies: Vec<_> = cols.iter().map(|_| proptest::option::of(value())).collect();
    slot_strategies.prop_map(move |slots| {
        cols.iter()
            .cloned()
            .zip(slots)
            .filter_map(|(c, v)| v.map(|v| (c, v)))
            .collect()
    })
}

/// A whole table: pick a small column universe, then rows drawn over it.
/// `Table::from_rows` re-derives the sorted column union, so the parsed table's
/// columns are exactly the sorted keys that actually appear — which is what we
/// compare against.
fn table_strategy() -> impl Strategy<Value = Table> {
    // Column universe (deduped by BTreeMap key), 0..5 columns.
    btree_map(column_key(), Just(()), 0..5)
        .prop_flat_map(|universe| {
            let cols: Vec<String> = universe.into_keys().collect();
            let row_strat = row(cols.clone());
            vec(row_strat, 0..6)
        })
        .prop_map(|rows| Table::from_rows("rows", rows).expect("`rows` is a valid table name"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn table_roundtrip_is_identity(table in table_strategy()) {
        let rendered = table
            .render()
            .unwrap_or_else(|e| panic!("render failed: {e}\ntable: {table:?}"));
        let parsed = Table::parse(&rendered)
            .unwrap_or_else(|e| panic!("parse failed: {e}\nrendered:\n{rendered}"));
        prop_assert_eq!(parsed, table, "round-trip mismatch\nrendered:\n{}", rendered);
    }

    #[test]
    fn render_always_parses(table in table_strategy()) {
        let rendered = table.render().expect("render");
        prop_assert!(Table::parse(&rendered).is_ok(), "parse errored on:\n{}", rendered);
    }
}
