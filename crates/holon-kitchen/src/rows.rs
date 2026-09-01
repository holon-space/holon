//! The `recipe` / `ingredient_use` rows a `.cook` file owns.
//!
//! Ids are DERIVED, never minted: a recipe's id is its vault-relative path, and
//! an ingredient use's is that path plus the ingredient's own name, so an
//! ingredient added at the top cannot re-point every id below it at a different
//! ingredient; position survives only in the occurrence counter that separates
//! two uses whose names reduce to the SAME slug, which reordering those two
//! does swap.

use std::collections::HashMap;

use anyhow::Result;
use anyhow::bail;
use cooklang::Recipe;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::file_format::TypedRowSet;

/// The two declared types a recipe file writes, in the order a reader wants
/// them: the recipe exists before the uses that name it.
pub(crate) fn recipe_row_sets(
    recipe: &Recipe,
    rel_path: &str,
    title: &str,
    course: Option<String>,
) -> Result<Vec<TypedRowSet>> {
    checked_local_id("recipe", rel_path)?;

    let mut recipe_row = StorageEntity::new();
    recipe_row.insert("id".into(), Value::String(rel_path.to_string()));
    recipe_row.insert("source_path".into(), Value::String(rel_path.to_string()));
    recipe_row.insert("title".into(), Value::String(title.to_string()));
    // `servings` is deliberately not written: cooklang admits non-integer
    // servings (`4|6|8`) that the INTEGER column cannot hold, and the metadata
    // reaches the recipe page through the document block's properties either
    // way.
    recipe_row.insert(
        "course".into(),
        course.map(Value::String).unwrap_or(Value::Null),
    );

    // The id this row LANDS under: the write path prefixes a supplied id with
    // the entity's canonical (hyphenated) name. Every reference to the recipe
    // must use that form.
    let recipe_id = format!("recipe:{rel_path}");

    let mut uses = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for use_ in crate::cook::uses_of(recipe)? {
        let slug = id_slug(&use_.name);
        let occurrence = seen.entry(slug.clone()).or_insert(0);
        let local = format!("{rel_path}::iu::{slug}-{occurrence}");
        *occurrence += 1;
        checked_local_id("ingredient-use", &local)?;

        let mut row = StorageEntity::new();
        row.insert("id".into(), Value::String(local));
        row.insert("recipe_id".into(), Value::String(recipe_id.clone()));
        // The schema's column for what the parser calls `name` (Kitchen.md D6).
        row.insert("raw_name".into(), Value::String(use_.name));
        row.insert(
            "quantity".into(),
            use_.quantity.map(Value::Float).unwrap_or(Value::Null),
        );
        row.insert(
            "unit".into(),
            use_.unit.map(Value::String).unwrap_or(Value::Null),
        );
        row.insert("step_index".into(), Value::Integer(use_.step_index as i64));
        uses.push(row);
    }

    Ok(vec![
        TypedRowSet {
            type_name: "recipe".to_string(),
            owner_column: "source_path".to_string(),
            owner_value: rel_path.to_string(),
            rows: vec![recipe_row],
        },
        TypedRowSet {
            type_name: "ingredient_use".to_string(),
            owner_column: "recipe_id".to_string(),
            owner_value: recipe_id,
            rows: uses,
        },
    ])
}

/// An ingredient name reduced to URI-path characters, so `@crème fraîche` and
/// `@sea salt` yield ids the write path can actually store.
///
/// Distinct names may collide here (`sea salt` and `sea-salt`); the occurrence
/// counter in the caller is keyed on the SLUG, not the name, so a collision
/// still produces distinct ids rather than one row overwriting the other.
fn id_slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Refuse a derived id that would not LAND as `{entity}:{local}`.
///
/// The check is the write path's own `from_raw_for`, so it catches both silent
/// failures at once: a file name the URI grammar rejects (a space) would panic
/// inside a spawned ingest task, and one that parses as an ALREADY-schemed URI
/// (a `:` in the path) would be stored unprefixed, leaving every reference to
/// it joining to nothing.
fn checked_local_id(entity: &str, local: &str) -> Result<()> {
    let intended = format!("{entity}:{local}");
    if EntityUri::parse(&intended).is_err() {
        bail!(
            "derived {entity} id {local:?} is not a storable URI path. Rename the file to one \
             the id grammar admits."
        );
    }
    let landed = EntityUri::from_raw_for(entity, local).to_string();
    if landed != intended {
        bail!(
            "derived {entity} id {local:?} would land as {landed:?} rather than {intended:?} — it \
             already reads as a schemed URI, so it is stored unprefixed and every reference to it \
             joins to nothing. Rename the file."
        );
    }
    Ok(())
}
