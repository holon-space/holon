//! Differential: the rows the cooklang plugin produces survive the neutral
//! JSON-Lines contract unchanged.
//!
//! The reference is the adapter's own `typed_rows`; the SUT is those rows put
//! through `holon_rows::emit_row_sets` → `parse_row_sets`. The plugin speaks
//! that contract on the wire, so anything the contract cannot carry today is a
//! row the plugin would silently lose.
//!
//! Only one `.cook` fixture exists, so the recipes come from a generator: the
//! interesting shapes — a recipe with NO ingredients (an empty
//! `ingredient_use` scope), a bare `@salt` (NULL quantity AND unit), two uses
//! whose names share a slug — are all rare in hand-written fixtures and all
//! load-bearing for the contract.

use std::path::PathBuf;
use std::sync::LazyLock;

use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::TypedRowSet;
use holon_plugin_host::PluginFormatAdapter;
use holon_rows::emit_row_sets;
use holon_rows::parse_row_sets;
use proptest::prelude::*;

mod support;

/// One guest instance for the whole run — instantiating per case would dwarf
/// the parse the property is about.
static PLUGIN: LazyLock<PluginFormatAdapter> = LazyLock::new(support::bundled_cook_plugin);

fn rows_of(rel: &str, content: &str) -> Vec<TypedRowSet> {
    let root = PathBuf::from("/vault");
    PLUGIN
        .parse(&root.join(rel), content, &EntityUri::no_parent(), &root)
        .expect("the generator only produces recipes the adapter accepts")
        .typed_rows
}

fn round_trip(sets: &[TypedRowSet]) -> Vec<TypedRowSet> {
    let text = emit_row_sets(sets).expect("adapter rows must be emittable");
    parse_row_sets(&text).expect("our own stream must parse")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    /// Nothing the adapter emits is lost, added or re-typed by the contract.
    #[test]
    fn cook_rows_survive_the_json_lines_contract(text in support::recipe_text()) {
        let sets = rows_of("Rezepte/Generated.cook", &text);
        prop_assert_eq!(round_trip(&sets), sets);
    }

    /// Both scopes reach the wire even when one owns no rows — an
    /// `ingredient_use` scope dropped for being empty is how the LAST
    /// ingredient of a recipe would never get swept on re-ingest.
    #[test]
    fn both_scopes_reach_the_wire(text in support::recipe_text()) {
        let sets = rows_of("Rezepte/Generated.cook", &text);
        let back = round_trip(&sets);
        prop_assert_eq!(
            back.iter().map(|s| s.type_name.as_str()).collect::<Vec<_>>(),
            vec!["recipe", "ingredient_use"]
        );
    }
}

#[test]
fn the_pancakes_fixture_survives_the_contract() {
    let content = support::pancakes_fixture();
    let sets = rows_of("pancakes.cook", &content);
    assert_eq!(round_trip(&sets), sets);
}

/// `@maple syrup{}` has no amount, and the row must carry NULL for it — not a
/// fabricated zero, and not an absent column, either of which would let a
/// nutrition rollup read the ingredient as weightless.
#[test]
fn an_amountless_ingredient_keeps_its_null_quantity_on_the_wire() {
    let content = support::pancakes_fixture();
    let sets = rows_of("pancakes.cook", &content);
    let text = emit_row_sets(&sets).unwrap();

    let syrup = text
        .lines()
        .find(|line| line.contains("maple syrup"))
        .expect("the fixture's amountless ingredient must reach the wire");
    assert!(
        syrup.contains("\"quantity\":null"),
        "quantity must be null, not zero or absent: {syrup}"
    );

    let back = round_trip(&sets);
    let uses = back
        .iter()
        .find(|s| s.type_name == "ingredient_use")
        .unwrap();
    let row = uses
        .rows
        .iter()
        .find(|r| r.get("raw_name") == Some(&holon_api::Value::String("maple syrup".into())))
        .unwrap();
    assert_eq!(row.get("quantity"), Some(&holon_api::Value::Null));
    assert_eq!(row.get("unit"), Some(&holon_api::Value::Null));
}
