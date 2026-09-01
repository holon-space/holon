//! Kitchen Inc B keystone — "what can I cook now".
//!
//! A recipe is cookable iff EVERY one of its `ingredient_use` rows is covered
//! by a `pantry_item` with sufficient quantity in the SAME unit. The predicate
//! is a QUERY, never a computed field: it is an aggregate in disguise ("all
//! children satisfy…") and expressing it as a field would front-run Inc D's
//! aggregate-language growth (docs/Plans/Kitchen.md §5 Inc B).
//!
//! Unit conversion does not exist yet — `product.density_g_per_ml` is an Inc D
//! type. Until then a differing-unit pair is UNCONVERTIBLE: it makes the recipe
//! not cookable AND surfaces as a named blocker, never as a silent skip and
//! never as "satisfied" (Kitchen.md §3.2 D1).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use holon::testing::e2e_test_helpers::E2ETestContext;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_api::streaming::Change;
use holon_core::storage::types::StorageEntity;
use holon_kitchen::COOK_BLOCKERS_SQL;
use holon_kitchen::COOKABLE_RECIPES_SQL;
use holon_kitchen::CookBlockReason;

/// The id a `create` actually lands under: the write path prefixes the caller's
/// id with the entity's canonical (kebab) name, so `p-flour` on `pantry_item`
/// is stored as `pantry-item:p-flour`.
///
/// Every cross-table reference must use the MINTED form. This is the shape the
/// ingredient-use persistence leg has to write too — a `recipe_id` holding the
/// bare id joins to nothing, silently and with no error.
fn minted(entity: &str, local: &str) -> String {
    format!("{}:{local}", entity.replace('_', "-"))
}

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

async fn add_recipe(ctx: &E2ETestContext, id: &str, title: &str) -> Result<()> {
    ctx.execute_op(
        "recipe",
        "create",
        params(&[
            ("id", Value::String(id.to_string())),
            ("title", Value::String(title.to_string())),
            ("source_path", Value::String(format!("{title}.cook"))),
        ]),
    )
    .await
}

/// One ingredient the recipe requires. `unit`/`quantity` are `None` for a bare
/// `@salt`, which cooklang admits and no REAL column can hold.
async fn require(
    ctx: &E2ETestContext,
    id: &str,
    recipe_id: &str,
    raw_name: &str,
    quantity: Option<f64>,
    unit: Option<&str>,
) -> Result<()> {
    ctx.execute_op(
        "ingredient_use",
        "create",
        params(&[
            ("id", Value::String(id.to_string())),
            ("recipe_id", Value::String(minted("recipe", recipe_id))),
            ("raw_name", Value::String(raw_name.to_string())),
            (
                "quantity",
                quantity.map(Value::Float).unwrap_or(Value::Null),
            ),
            (
                "unit",
                unit.map(|u| Value::String(u.to_string()))
                    .unwrap_or(Value::Null),
            ),
            ("step_index", Value::Integer(1)),
        ]),
    )
    .await
}

/// The `add` half of Inc B's pantry ops — the generic `create` the declared
/// type already advertises, not a bespoke second door.
async fn stock(
    ctx: &E2ETestContext,
    id: &str,
    name: &str,
    quantity: f64,
    unit: Option<&str>,
) -> Result<()> {
    ctx.execute_op(
        "pantry_item",
        "create",
        params(&[
            ("id", Value::String(id.to_string())),
            ("name", Value::String(name.to_string())),
            ("quantity", Value::Float(quantity)),
            (
                "unit",
                unit.map(|u| Value::String(u.to_string()))
                    .unwrap_or(Value::Null),
            ),
        ]),
    )
    .await
}

async fn consume(ctx: &E2ETestContext, id: &str, quantity: f64, unit: Option<&str>) -> Result<()> {
    ctx.execute_op(
        "pantry_item",
        "consume",
        params(&[
            ("id", Value::String(minted("pantry_item", id))),
            ("quantity", Value::Float(quantity)),
            (
                "unit",
                unit.map(|u| Value::String(u.to_string()))
                    .unwrap_or(Value::Null),
            ),
        ]),
    )
    .await
}

async fn cookable_titles(ctx: &E2ETestContext) -> Result<Vec<String>> {
    let rows = ctx
        .query(
            COOKABLE_RECIPES_SQL.to_string(),
            QueryLanguage::HolonSql,
            HashMap::new(),
        )
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.get("title").and_then(|v| v.as_string()).map(String::from))
        .collect())
}

/// Every blocker for `recipe_id`, as `(raw_name, reason)`, reason PARSED — a
/// reason string the query can emit but the enum cannot name is a fail-loud
/// desync between the SQL and the Rust seat, not something to default away.
async fn blockers(ctx: &E2ETestContext, recipe_id: &str) -> Result<Vec<(String, CookBlockReason)>> {
    let rows = ctx
        .query(
            COOK_BLOCKERS_SQL.to_string(),
            QueryLanguage::HolonSql,
            HashMap::new(),
        )
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let of_recipe = row
            .get("recipe_id")
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string();
        if of_recipe != minted("recipe", recipe_id) {
            continue;
        }
        let raw_name = row
            .get("raw_name")
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string();
        let reason_text = row
            .get("reason")
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("blocker row for {raw_name} carries no reason"))?;
        out.push((raw_name, CookBlockReason::parse(reason_text)?));
    }
    out.sort();
    Ok(out)
}

/// The keystone: stock the pantry and the recipe becomes cookable; consume an
/// ingredient below what the recipe needs and it leaves the list again.
#[tokio::test(flavor = "multi_thread")]
async fn stocking_the_pantry_makes_a_recipe_cookable_and_consuming_removes_it() -> Result<()> {
    let ctx = E2ETestContext::new().await?;

    add_recipe(&ctx, "r-pancakes", "Pancakes").await?;
    require(&ctx, "iu-1", "r-pancakes", "flour", Some(200.0), Some("g")).await?;
    require(&ctx, "iu-2", "r-pancakes", "milk", Some(300.0), Some("ml")).await?;

    assert_eq!(
        cookable_titles(&ctx).await?,
        Vec::<String>::new(),
        "an empty pantry cooks nothing"
    );
    assert_eq!(
        blockers(&ctx, "r-pancakes").await?,
        vec![
            ("flour".to_string(), CookBlockReason::Missing),
            ("milk".to_string(), CookBlockReason::Missing),
        ],
        "both ingredients are absent from the pantry and must say so by name"
    );

    stock(&ctx, "p-flour", "flour", 500.0, Some("g")).await?;
    stock(&ctx, "p-milk", "milk", 1000.0, Some("ml")).await?;

    assert_eq!(
        cookable_titles(&ctx).await?,
        vec!["Pancakes".to_string()],
        "with both ingredients stocked in sufficient quantity the recipe is cookable"
    );
    assert!(
        blockers(&ctx, "r-pancakes").await?.is_empty(),
        "a cookable recipe has no blockers"
    );

    // 500 - 400 = 100 g of flour left, but the recipe needs 200 g.
    consume(&ctx, "p-flour", 400.0, Some("g")).await?;

    assert_eq!(
        cookable_titles(&ctx).await?,
        Vec::<String>::new(),
        "consuming flour below what the recipe needs takes it off the list"
    );
    assert_eq!(
        blockers(&ctx, "r-pancakes").await?,
        vec![("flour".to_string(), CookBlockReason::Insufficient)],
        "the short ingredient is named, and named as short rather than as missing"
    );

    Ok(())
}

/// A unit we cannot convert is NOT a silent skip and NOT "satisfied". Real
/// conversion needs `product.density_g_per_ml` (Inc D); until then the honest
/// answer is a visible refusal.
#[tokio::test(flavor = "multi_thread")]
async fn an_unconvertible_unit_blocks_the_recipe_by_name() -> Result<()> {
    let ctx = E2ETestContext::new().await?;

    add_recipe(&ctx, "r-cake", "Cake").await?;
    require(&ctx, "iu-c1", "r-cake", "sugar", Some(100.0), Some("g")).await?;

    // The NUMBER is larger than what the recipe asks for, so only the unit rule
    // can block this. A smaller number would let the quantity test mask the
    // conversion test and the assertion would pass without the rule existing.
    stock(&ctx, "p-sugar", "sugar", 500.0, Some("kg")).await?;

    assert_eq!(
        cookable_titles(&ctx).await?,
        Vec::<String>::new(),
        "an unconvertible unit must never read as satisfied"
    );
    assert_eq!(
        blockers(&ctx, "r-cake").await?,
        vec![("sugar".to_string(), CookBlockReason::Unconvertible)],
        "and it must be distinguishable from simply missing or being short"
    );

    Ok(())
}

/// A bare `@salt` carries no amount, so presence is the whole test. Without
/// this the ingredient could never be satisfied and every recipe using one
/// would be permanently uncookable.
#[tokio::test(flavor = "multi_thread")]
async fn an_ingredient_with_no_amount_is_satisfied_by_presence() -> Result<()> {
    let ctx = E2ETestContext::new().await?;

    add_recipe(&ctx, "r-eggs", "Fried Eggs").await?;
    require(&ctx, "iu-e1", "r-eggs", "salt", None, None).await?;

    assert_eq!(cookable_titles(&ctx).await?, Vec::<String>::new());

    stock(&ctx, "p-salt", "salt", 1.0, None).await?;

    assert_eq!(
        cookable_titles(&ctx).await?,
        vec!["Fried Eggs".to_string()],
        "any salt at all satisfies an amount-less ingredient"
    );

    Ok(())
}

/// A recipe whose ingredients we have not parsed is NOT vacuously cookable.
/// `NOT EXISTS(unsatisfied)` over zero children is trivially true, which would
/// declare every recipe with no `ingredient_use` rows ready to cook — the exact
/// false green that hides missing ingest.
#[tokio::test(flavor = "multi_thread")]
async fn a_recipe_with_no_known_ingredients_is_not_cookable() -> Result<()> {
    let ctx = E2ETestContext::new().await?;

    add_recipe(&ctx, "r-mystery", "Mystery Stew").await?;
    stock(&ctx, "p-everything", "everything", 999.0, Some("g")).await?;

    assert_eq!(
        cookable_titles(&ctx).await?,
        Vec::<String>::new(),
        "no parsed ingredients means unknown, and unknown is not cookable"
    );

    Ok(())
}

/// Consuming more than is on hand is a refusal, not a negative balance. A
/// pantry that can go below zero silently reports recipes as uncookable
/// forever with no way to see why.
#[tokio::test(flavor = "multi_thread")]
async fn consuming_more_than_is_on_hand_is_refused() -> Result<()> {
    let ctx = E2ETestContext::new().await?;
    stock(&ctx, "p-flour", "flour", 100.0, Some("g")).await?;

    let err = consume(&ctx, "p-flour", 250.0, Some("g"))
        .await
        .expect_err("consuming past empty must fail loudly");
    let text = format!("{err:#}");
    assert!(
        text.contains("100") && text.contains("250"),
        "the refusal must name what was on hand and what was asked for, got: {text}"
    );

    Ok(())
}

/// Consuming in a unit we cannot convert is refused rather than applied as if
/// the numbers were comparable.
#[tokio::test(flavor = "multi_thread")]
async fn consuming_in_an_unconvertible_unit_is_refused() -> Result<()> {
    let ctx = E2ETestContext::new().await?;
    stock(&ctx, "p-flour", "flour", 1000.0, Some("g")).await?;

    let err = consume(&ctx, "p-flour", 1.0, Some("kg"))
        .await
        .expect_err("a kg consume against a g pantry item must fail loudly");
    let text = format!("{err:#}");
    assert!(
        text.contains("kg") && text.contains('g'),
        "the refusal must name both units, got: {text}"
    );

    Ok(())
}

/// The list is LIVE: a watcher sees the recipe arrive when the pantry is
/// stocked, without re-asking.
///
/// The predicate is a subquery shape, so it is served by disclosed eager
/// re-execution rather than an incrementally maintained matview. That
/// disclosure is asserted here — degraded mode is acceptable, degraded mode
/// that does not say so is not.
#[tokio::test(flavor = "multi_thread")]
async fn the_cookable_list_updates_live_and_discloses_its_degraded_mode() -> Result<()> {
    let ctx = E2ETestContext::new().await?;

    add_recipe(&ctx, "r-toast", "Toast").await?;
    require(&ctx, "iu-t1", "r-toast", "bread", Some(2.0), Some("slice")).await?;

    let stream = ctx
        .query_and_watch(
            COOKABLE_RECIPES_SQL.to_string(),
            QueryLanguage::HolonSql,
            HashMap::new(),
        )
        .await?;

    stock(&ctx, "p-bread", "bread", 10.0, Some("slice")).await?;

    let batches = ctx
        .collect_stream_events(stream, Duration::from_secs(10), None)
        .await?;

    assert!(
        batches.iter().any(|b| b.metadata.degraded.is_some()),
        "a query served by re-execution must disclose it"
    );

    let saw_toast = batches.iter().any(|batch| {
        batch.items.iter().any(|row| {
            let data = match &row.change {
                Change::Created { data, .. } | Change::Updated { data, .. } => data,
                _ => return false,
            };
            data.get("title")
                .and_then(|v| v.as_string())
                .is_some_and(|t| t == "Toast")
        })
    });
    assert!(
        saw_toast,
        "stocking bread must push Toast onto the live cookable list; batches: {batches:#?}"
    );

    Ok(())
}
