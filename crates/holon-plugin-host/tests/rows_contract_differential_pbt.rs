//! Differential: the rows the host stores for a recipe are, cell for cell, the
//! rows the cooklang guest emitted.
//!
//! The reference is the guest's own stream, read with `Stream::from_jsonl` and
//! mapped by the contract's rules (ADR 0034 section 2): each id and reference
//! is its `LocalId` rendered under the type's scheme, each cell is its JSON
//! value. The SUT is the adapter's `typed_rows` for the same file.
//!
//! The recipes come from a generator because the load-bearing shapes are rare
//! in hand-written fixtures: a recipe with NO ingredients (an empty
//! `ingredient_use` scope), a bare `@salt` (NULL quantity AND unit), two uses
//! whose names share a slug.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::Mutex;

use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::TypedRowSet;
use holon_plugin_host::BUNDLED_PLUGINS;
use holon_plugin_host::PluginFormatAdapter;
use holon_plugin_host::PluginHost;
use holon_plugin_host::PluginLimits;
use holon_plugin_rows::Line;
use holon_plugin_rows::LocalId;
use holon_plugin_rows::Owner;
use holon_plugin_rows::Stream;
use proptest::prelude::*;

mod support;

/// One instance of each for the whole run — instantiating per case would
/// dwarf the parse the property is about.
static PLUGIN: LazyLock<PluginFormatAdapter> = LazyLock::new(support::bundled_cook_plugin);
static GUEST: LazyLock<Mutex<PluginHost>> = LazyLock::new(|| {
    Mutex::new(
        PluginHost::from_bytes(BUNDLED_PLUGINS[0].guest_wasm, PluginLimits::default())
            .expect("the bundled cooklang guest must instantiate"),
    )
});

/// The columns `plugins/cooklang.yaml` declares for `ingredient_use`.
const INGREDIENT_USE_COLUMNS: [&str; 6] = [
    "id",
    "recipe_id",
    "raw_name",
    "quantity",
    "unit",
    "step_index",
];

fn guest_stream(source_path: &str, file_stem: &str, content: &str) -> Stream {
    let ctx = serde_json::json!({ "source_path": source_path, "file_stem": file_stem });
    let text = GUEST
        .lock()
        .unwrap()
        .parse(content.as_bytes(), ctx.to_string().as_bytes())
        .unwrap_or_else(|e| panic!("the guest must accept {source_path:?}: {e}"));
    Stream::from_jsonl(&text).unwrap_or_else(|e| panic!("the guest's stream must parse: {e}"))
}

fn stored_rows(source_path: &str, content: &str) -> Vec<TypedRowSet> {
    let root = PathBuf::from("/vault");
    PLUGIN
        .parse(
            &root.join(source_path),
            content,
            &EntityUri::no_parent(),
            &root,
        )
        .unwrap_or_else(|e| panic!("{source_path:?} must ingest: {e:#}"))
        .typed_rows
}

fn uri(type_name: &str, id: &LocalId) -> String {
    EntityUri::from_segments(&type_name.replace('_', "-"), id.path_segments(), id.parts())
        .to_string()
}

fn cell(value: serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Float(n.as_f64().expect("a JSON number is an i64 or an f64")),
        },
        other => panic!("the cooklang guest emits no cell like {other}"),
    }
}

/// What the contract says the host stores for `stream`.
fn expected_rows(stream: &Stream) -> Vec<TypedRowSet> {
    stream
        .scopes
        .iter()
        .map(|scope| TypedRowSet {
            type_name: scope.type_name.clone(),
            owner_column: scope.owner_column.clone(),
            owner_value: match &scope.owner {
                Owner::Text(text) => text.clone(),
                Owner::Ref(target) => uri(&target.type_name, &target.id),
            },
            rows: stream
                .lines
                .iter()
                .filter_map(|line| match line {
                    Line::Row(row) if row.type_name == scope.type_name => Some(row),
                    _ => None,
                })
                .map(|row| {
                    let mut entity = StorageEntity::new();
                    entity.insert("id".into(), Value::String(uri(&row.type_name, &row.id)));
                    for (column, target) in row.refs.iter() {
                        entity.insert(
                            column.into(),
                            Value::String(uri(&target.type_name, &target.id)),
                        );
                    }
                    for (column, value) in row.cells.iter() {
                        entity.insert(column.into(), cell(value.clone()));
                    }
                    entity
                })
                .collect(),
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    /// Every row, reference and cell the guest emits is stored as emitted:
    /// nothing lost, added, re-typed or moved to another scope.
    #[test]
    fn stored_rows_are_the_guest_rows_cell_for_cell(
        name in "[^/.\\x00][^/\\x00]{0,10}",
        text in support::recipe_text(),
    ) {
        let file = format!("{name}.cook");
        let source_path = format!("Rezepte/{file}");
        let stream = guest_stream(&source_path, &name, &text);
        prop_assert_eq!(Stream::from_jsonl(&stream.to_jsonl()), Ok(stream.clone()));

        let stored = stored_rows(&source_path, &text);
        prop_assert_eq!(&stored, &expected_rows(&stream));

        // Both scopes are stored even when one owns no rows: an
        // `ingredient_use` scope dropped for being empty is how the LAST
        // ingredient of a recipe would never get swept on re-ingest.
        prop_assert_eq!(
            stored.iter().map(|s| s.type_name.as_str()).collect::<Vec<_>>(),
            vec!["recipe", "ingredient_use"]
        );
        for set in &stored {
            let ids: BTreeSet<String> = set.rows.iter().map(|r| format!("{:?}", r["id"])).collect();
            prop_assert_eq!(ids.len(), set.rows.len(), "two {} rows share an id", set.type_name);
        }
        // A bare `@salt` stores NULL quantity and unit, not absent columns: an
        // absent one would let a nutrition rollup read the ingredient as
        // weightless.
        for row in &stored[1].rows {
            let columns: BTreeSet<&str> = row.keys().map(|k| &**k).collect();
            prop_assert_eq!(columns, BTreeSet::from(INGREDIENT_USE_COLUMNS));
        }
    }
}

/// `@maple syrup{}` has no amount, and the row must carry NULL for it — not a
/// fabricated zero.
#[test]
fn an_amountless_ingredient_stores_null_quantity_and_unit() {
    let stored = stored_rows("pancakes.cook", &support::pancakes_fixture());
    let syrup = stored[1]
        .rows
        .iter()
        .find(|r| r.get("raw_name") == Some(&Value::String("maple syrup".into())))
        .expect("the fixture's amountless ingredient must be stored");
    assert_eq!(syrup.get("quantity"), Some(&Value::Null));
    assert_eq!(syrup.get("unit"), Some(&Value::Null));
}
