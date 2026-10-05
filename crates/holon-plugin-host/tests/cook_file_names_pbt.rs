//! A recipe ingests whatever its file is called, and every id it projects is
//! the host's encoding of that path.
//!
//! The file name is the user's: spaces, umlauts, `:` and `%` all occur in a
//! real vault. The guest names ids only as path segments and parts; the host
//! renders them, so a name the URI grammar would reject is still a storable id.

use std::path::PathBuf;
use std::sync::LazyLock;

use holon_api::EntityUri;
use holon_api::Value;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_plugin_host::PluginFormatAdapter;
use proptest::prelude::*;

mod support;

/// One guest instance for the whole run — instantiating per case would dwarf
/// the parse the property is about.
static PLUGIN: LazyLock<PluginFormatAdapter> = LazyLock::new(support::bundled_cook_plugin);

fn parse(rel: &str, content: &str) -> FileFormatParseResult {
    let root = PathBuf::from("/vault");
    PLUGIN
        .parse(&root.join(rel), content, &EntityUri::no_parent(), &root)
        .unwrap_or_else(|e| panic!("{rel:?} must ingest: {e:#}"))
}

fn id_of(row: &holon_api::StorageEntity) -> &str {
    match row.get("id") {
        Some(Value::String(id)) => id,
        other => panic!("a row carries id {other:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Every id the recipe projects is its path encoded segment by segment:
    /// the recipe row, each ingredient use, the reference from a use to its
    /// recipe, the scope owner and the step blocks.
    #[test]
    fn any_file_name_ingests_under_its_encoded_path(
        name in "[^/.\\x00][^/\\x00]{0,10}",
        text in support::recipe_text(),
    ) {
        let file = format!("{name}.cook");
        let parsed = parse(&format!("Rezepte/{file}"), &text);
        let segments = ["Rezepte", file.as_str()];
        let recipe = EntityUri::from_segments("recipe", &segments, &[] as &[&str]).to_string();

        let types: Vec<&str> = parsed.typed_rows.iter().map(|s| s.type_name.as_str()).collect();
        prop_assert_eq!(types, vec!["recipe", "ingredient_use"]);
        let recipes = &parsed.typed_rows[0];
        let uses = &parsed.typed_rows[1];

        prop_assert_eq!(recipes.rows.len(), 1);
        prop_assert_eq!(id_of(&recipes.rows[0]), recipe.as_str());
        prop_assert_eq!(&uses.owner_value, &recipe);
        for row in &uses.rows {
            prop_assert_eq!(row.get("recipe_id"), Some(&Value::String(recipe.clone())));
            let prefix = format!(
                "{}::iu::",
                EntityUri::from_segments("ingredient-use", &segments, &[] as &[&str])
            );
            prop_assert!(id_of(row).starts_with(&prefix), "{} lacks {}", id_of(row), prefix);
        }
        for (seq, block) in parsed.blocks.iter().enumerate() {
            let expected = EntityUri::from_segments("block", &segments, &["b".to_string(), seq.to_string()]);
            prop_assert_eq!(&block.id, &expected);
        }
    }
}
