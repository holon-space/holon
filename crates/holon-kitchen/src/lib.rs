//! Kitchen domain: recipes and the shopping list, and (later) pantry and
//! nutrition.
//!
//! Two read legs today. The cooklang adapter projects `.cook` files, which are
//! authoritative and are never written back here. [`shopping`] pulls the
//! shopping-list peer, which owns its own list and issues no item ids, so this
//! crate carries the identity and reconciliation the generic entity mirror
//! cannot supply.
//!
//! Per-format de-risking: this crate exercises the existing Block-shaped
//! `FileFormatAdapter` on a second, non-org format — extension claiming,
//! document identity, and read authority. It deliberately does NOT generalize
//! `FileFormatAdapter`/`FileFormatParseResult` to be type-generic; that is BG
//! Inc-5's C7 and stays out of scope (see docs/Plans/Kitchen.md R1).

pub mod cook;
pub mod cookable;
pub mod file_format;
pub mod params;
mod rows;
pub mod shopping;
pub mod shopping_sync;

use anyhow::Context as _;
use anyhow::Result;
pub use cook::IngredientUse;
pub use cook::STEP_NUMBER_KEY;
pub use cook::ingredient_uses;
pub use cook::parse_recipe;
pub use cookable::COOK_BLOCKERS_SQL;
pub use cookable::COOKABLE_RECIPES_SQL;
pub use cookable::CookBlockReason;
pub use file_format::CookFormatAdapter;
use holon_api::entity::TypeDefinition;
use holon_profiles::parse_profile_yaml;
use holon_profiles::type_registry::TypeRegistry;

pub const RECIPE_TYPE_YAML: &str = include_str!("../assets/types/recipe.yaml");
pub const INGREDIENT_USE_TYPE_YAML: &str = include_str!("../assets/types/ingredient_use.yaml");
pub const PANTRY_ITEM_TYPE_YAML: &str = include_str!("../assets/types/pantry_item.yaml");
pub const RECIPE_PROFILE_YAML: &str = include_str!("../assets/types/recipe_profile.yaml");
pub const SHOPPING_ITEM_TYPE_YAML: &str = include_str!("../assets/types/shopping_item.yaml");
pub const SHOPPING_ITEM_PROFILE_YAML: &str =
    include_str!("../assets/types/shopping_item_profile.yaml");

/// The `shopping_item` declaration, parsed. The sync leg reads its
/// `soft_delete` retention from here rather than carrying a constant of its
/// own: the tombstone window is a property of the declared type, and two
/// spellings of it would let a yaml edit silently disagree with the reconciler.
pub fn shopping_item_type() -> Result<TypeDefinition> {
    serde_yaml::from_str(SHOPPING_ITEM_TYPE_YAML)
        .context("Failed to parse the shopping_item type declaration")
}

/// Register the kitchen datatypes and the recipe page's render profile.
///
/// Mirrors `create_default_registry`'s bundled-type loop: types first, then the
/// profile that augments one of them.
pub fn register_kitchen_types(registry: &TypeRegistry) -> Result<()> {
    for (name, yaml) in [
        ("recipe", RECIPE_TYPE_YAML),
        ("ingredient_use", INGREDIENT_USE_TYPE_YAML),
        ("pantry_item", PANTRY_ITEM_TYPE_YAML),
        ("shopping_item", SHOPPING_ITEM_TYPE_YAML),
    ] {
        let type_def: TypeDefinition = serde_yaml::from_str(yaml)
            .with_context(|| format!("Failed to parse kitchen type '{name}'"))?;
        registry
            .register(type_def)
            .with_context(|| format!("Failed to register kitchen type '{name}'"))?;
    }

    for (name, yaml) in [
        ("recipe", RECIPE_PROFILE_YAML),
        ("shopping_item", SHOPPING_ITEM_PROFILE_YAML),
    ] {
        let profile = parse_profile_yaml(yaml)
            .with_context(|| format!("Failed to parse the {name} profile YAML"))?;
        // Fail LOUD here if a render calls a lookup the engine never registers —
        // otherwise it errors at eval and degrades to () at WARN, inverting every
        // condition it feeds.
        holon_profiles::validate_lookups_registered(&profile).with_context(|| {
            format!("{name} profile references an unregistered lookup function")
        })?;
        registry
            .apply_parsed_profile(profile)
            .with_context(|| format!("Failed to apply the {name} profile"))?;
    }

    Ok(())
}
