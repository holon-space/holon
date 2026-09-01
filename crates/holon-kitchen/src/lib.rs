//! Kitchen domain: recipes, and (later) pantry, shopping and nutrition.
//!
//! Inc A scope — the cooklang READ adapter, the `recipe` / `ingredient_use`
//! datatypes, and the recipe page as the `recipe` type's default render
//! variant. `.cook` files in the vault are authoritative and are never written
//! back here.
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

/// Register the kitchen datatypes and the recipe page's render profile.
///
/// Mirrors `create_default_registry`'s bundled-type loop: types first, then the
/// profile that augments one of them.
pub fn register_kitchen_types(registry: &TypeRegistry) -> Result<()> {
    for (name, yaml) in [
        ("recipe", RECIPE_TYPE_YAML),
        ("ingredient_use", INGREDIENT_USE_TYPE_YAML),
        ("pantry_item", PANTRY_ITEM_TYPE_YAML),
    ] {
        let type_def: TypeDefinition = serde_yaml::from_str(yaml)
            .with_context(|| format!("Failed to parse kitchen type '{name}'"))?;
        registry
            .register(type_def)
            .with_context(|| format!("Failed to register kitchen type '{name}'"))?;
    }

    let profile = parse_profile_yaml(RECIPE_PROFILE_YAML)
        .context("Failed to parse the recipe profile YAML")?;
    // Fail LOUD here if a render calls a lookup the engine never registers —
    // otherwise it errors at eval and degrades to () at WARN, inverting every
    // condition it feeds.
    holon_profiles::validate_lookups_registered(&profile)
        .context("recipe profile references an unregistered lookup function")?;
    registry
        .apply_parsed_profile(profile)
        .context("Failed to apply the recipe profile")?;

    Ok(())
}
