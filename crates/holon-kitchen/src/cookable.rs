//! "What can I cook now" — the cookable-now predicate, as a QUERY.
//!
//! A recipe is cookable iff every one of its `ingredient_use` rows is covered
//! by a `pantry_item`. This is deliberately not a computed field: it is an
//! aggregate in disguise ("all children satisfy…"), and expressing it as a
//! field would force the aggregate language to grow a full increment early
//! (docs/Plans/Kitchen.md §5 Inc B).
//!
//! Both queries derive from ONE satisfaction predicate below, so the cookable
//! list and the blocker list cannot disagree about what "satisfied" means.

use std::sync::LazyLock;

use anyhow::Result;
use anyhow::bail;

/// Does pantry item `p` cover ingredient use `iu`?
///
/// Two rules carry the increment's whole conversion story:
/// - An ingredient with no amount (`@salt`, or `@salt{a pinch}`) is satisfied
///   by PRESENCE. It has no number to compare, and demanding one would make
///   every recipe using a bare ingredient permanently uncookable.
/// - Otherwise the units must be the SAME. Real conversion needs
///   `product.density_g_per_ml` / `unit_weight_g`, which are Inc D columns on a
///   type that does not exist yet. Until they do, `2 kg` does not satisfy `100
///   g` — not because that is true, but because inventing the factor is the
///   silent degradation the error ladder forbids (§3.2 D1). The unconvertible
///   pair surfaces by name in [`COOK_BLOCKERS_SQL`].
const SATISFIES: &str = "
    p.name = iu.raw_name
    AND (
        iu.quantity IS NULL
        OR (
            (p.unit = iu.unit OR (p.unit IS NULL AND iu.unit IS NULL))
            AND p.quantity >= iu.quantity
        )
    )
";

/// An `ingredient_use` row nothing in the pantry covers.
static UNSATISFIED: LazyLock<String> =
    LazyLock::new(|| format!("NOT EXISTS (SELECT 1 FROM pantry_item p WHERE {SATISFIES})"));

/// Recipes every ingredient of which is on hand.
///
/// The `EXISTS(ingredient_use)` clause is not redundant. `NOT
/// EXISTS(unsatisfied)` over zero children is vacuously TRUE, so without it
/// every recipe whose ingredients were never parsed would report as ready to
/// cook — a recipe we know nothing about would outrank one we have actually
/// checked.
pub static COOKABLE_RECIPES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT r.id AS recipe_id, r.title AS title
         FROM recipe r
         WHERE EXISTS (SELECT 1 FROM ingredient_use k WHERE k.recipe_id = r.id)
           AND NOT EXISTS (
               SELECT 1 FROM ingredient_use iu
               WHERE iu.recipe_id = r.id AND {}
           )
         ORDER BY r.title",
        &*UNSATISFIED
    )
});

/// Why each uncookable recipe is uncookable, one row per blocking ingredient.
///
/// This is the disclosure half of the feature. A recipe silently absent from
/// the cookable list is indistinguishable from one that is missing a single
/// pinch of salt; the blockers name which ingredient and which of the three
/// failures it is.
pub static COOK_BLOCKERS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT iu.recipe_id AS recipe_id,
                iu.id        AS ingredient_use_id,
                iu.raw_name  AS raw_name,
                iu.quantity  AS required_quantity,
                iu.unit      AS required_unit,
                CASE
                  WHEN NOT EXISTS (
                        SELECT 1 FROM pantry_item p WHERE p.name = iu.raw_name)
                    THEN '{missing}'
                  WHEN NOT EXISTS (
                        SELECT 1 FROM pantry_item p
                        WHERE p.name = iu.raw_name
                          AND (p.unit = iu.unit
                               OR (p.unit IS NULL AND iu.unit IS NULL)))
                    THEN '{unconvertible}'
                  ELSE '{insufficient}'
                END AS reason
         FROM ingredient_use iu
         WHERE {unsatisfied}
         ORDER BY iu.recipe_id, iu.raw_name",
        missing = CookBlockReason::Missing.as_str(),
        unconvertible = CookBlockReason::Unconvertible.as_str(),
        insufficient = CookBlockReason::Insufficient.as_str(),
        unsatisfied = &*UNSATISFIED,
    )
});

/// Why one ingredient keeps a recipe off the cookable list.
///
/// Parsed, not passed around as text: the three reasons drive different user
/// affordances (shop for it / stock more / teach us the conversion), and a
/// fourth string arriving from SQL is a desync between the two seats rather
/// than a case to fall through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CookBlockReason {
    /// The pantry holds nothing by that name.
    Missing,
    /// The pantry holds it, but in a unit we have no factor for.
    Unconvertible,
    /// The pantry holds it in the right unit, but not enough of it.
    Insufficient,
}

impl CookBlockReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unconvertible => "unconvertible",
            Self::Insufficient => "insufficient",
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "missing" => Self::Missing,
            "unconvertible" => Self::Unconvertible,
            "insufficient" => Self::Insufficient,
            other => bail!(
                "cookable-now blocker reason {other:?} is not one of missing/unconvertible/\
                 insufficient — the SQL seat and the Rust seat have diverged"
            ),
        })
    }
}
