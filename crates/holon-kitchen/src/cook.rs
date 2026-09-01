//! Cooklang → Holon projection.
//!
//! The `.cook` file is authoritative. Everything here reads it; nothing writes
//! it back (see [`crate::file_format`], Tier R/O).

use anyhow::Context as _;
use anyhow::Result;
use cooklang::Item;
use cooklang::Recipe;
use cooklang::model::Content;
use cooklang::quantity::Number;
use cooklang::quantity::Quantity;
use cooklang::quantity::Value as CookValue;

/// Property key carrying a step's 1-based number within its section. Its
/// presence is what distinguishes a step block from a prose paragraph.
pub const STEP_NUMBER_KEY: &str = "step_number";

/// One use of one ingredient by one recipe step.
///
/// `product_id` is the Inc D binding slot and is always `None` here. NULL means
/// UNMATCHED and stays visibly so — it is never silently treated as zero by a
/// nutrition rollup.
#[derive(Debug, Clone, PartialEq)]
pub struct IngredientUse {
    pub name: String,
    /// `None` for a bare ingredient (`@salt`) and for a non-numeric amount
    /// (`@salt{a pinch}`), which cooklang admits and no `REAL` column can hold.
    pub quantity: Option<f64>,
    pub unit: Option<String>,
    pub product_id: Option<String>,
    /// 1-based step number this ingredient is first referenced from.
    pub step_index: u32,
}

/// The `block_raw` storage columns, as one set built once.
static BLOCK_STORAGE_COLUMNS: std::sync::LazyLock<std::collections::HashSet<&'static str>> =
    std::sync::LazyLock::new(|| holon_api::schema::BLOCK.columns().into_iter().collect());

/// True for a metadata key that spells a `block_raw` STORAGE COLUMN.
///
/// Such a key is not a property: `partition_params` routes any param whose key
/// names a column straight to that column, so emitting one overwrites real row
/// state (`content` would replace the block's text, `id` its primary key).
pub(crate) fn names_block_storage_column(key: &str) -> bool {
    BLOCK_STORAGE_COLUMNS.contains(key)
}

/// Render one recipe-metadata value as the text a property holds.
///
/// A sequence of scalars — standard cooklang `tags: [quick, vegan]` — joins to
/// `"quick, vegan"`. Anything else (a nested mapping, a sequence of sequences)
/// has no single-column form, and per fail-loud policy is REFUSED by name
/// rather than silently skipped.
pub(crate) fn metadata_value_text(key: &str, value: &serde_yaml::Value) -> Result<String> {
    match value {
        serde_yaml::Value::String(s) => Ok(s.clone()),
        serde_yaml::Value::Number(n) => Ok(n.to_string()),
        serde_yaml::Value::Bool(b) => Ok(b.to_string()),
        serde_yaml::Value::Sequence(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    serde_yaml::Value::String(s) => parts.push(s.clone()),
                    serde_yaml::Value::Number(n) => parts.push(n.to_string()),
                    serde_yaml::Value::Bool(b) => parts.push(b.to_string()),
                    other => anyhow::bail!(
                        "cooklang metadata {key:?}: list entry {other:?} is not a scalar and has \
                         no single-column form"
                    ),
                }
            }
            Ok(parts.join(", "))
        }
        other => anyhow::bail!(
            "cooklang metadata {key:?}: value {other:?} is neither a scalar nor a list of \
             scalars and has no single-column form"
        ),
    }
}

/// Parse `source` as cooklang, failing loud with the format named.
pub fn parse_recipe(source: &str) -> Result<Recipe> {
    reject_unclosed_component_brace(source)?;
    let recipe = cooklang::parse(source)
        .into_result()
        .map(|(recipe, _warnings)| recipe)
        .map_err(|report| {
            anyhow::anyhow!(
                "cooklang parse failed: {}",
                // The report renders every diagnostic; keep them all rather
                // than surfacing only the first.
                report
                    .errors()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })
        .context("cooklang source is not a valid recipe")?;
    reject_swallowed_components(&recipe)?;
    Ok(recipe)
}

/// Refuse a component brace that never closes on its line.
///
/// cooklang accepts `@flour{200%g` and SILENTLY drops the quantity — no error,
/// no warning, an ingredient with no amount (measured at 0.18.7). Only an
/// unclosed OPEN brace is refused: a surplus `}` is ordinary prose
/// (`#pot{} and a } sign`) and must keep parsing.
fn reject_unclosed_component_brace(source: &str) -> Result<()> {
    for (n, line) in source.lines().enumerate() {
        let mut open_at: Option<usize> = None;
        for (col, ch) in line.char_indices() {
            match ch {
                '{' if open_at.is_none() => open_at = Some(col),
                '}' => open_at = None,
                _ => {}
            }
        }
        if open_at.is_some() {
            anyhow::bail!(
                "cooklang line {}: a component brace is never closed — cooklang would silently \
                 drop the quantity here: {line:?}",
                n + 1
            );
        }
    }
    Ok(())
}

/// Refuse a parsed component whose amount shows that its closing brace landed
/// too late and swallowed following text.
///
/// Measured at 0.18.7, all silent (valid, no warning), all brace-BALANCED, so
/// only the parsed amount reveals them:
/// - `@flour{200%g @sugar}` → unit `"g @sugar"`; `@sugar` gone entirely.
/// - `@flour{200 @sugar}` → VALUE `Text("200 @sugar")`, no unit at all.
/// - `#pot{1 @salt}` → cookware value `Text("1 @salt")`; the ingredient is
///   gone.
/// - `@flour{200%g with a }` → unit `"g with a"`.
///
/// Checked over ingredients AND cookware, across both halves of the amount:
/// 1. a component sigil in the unit OR in a text value — no real amount
///    contains one, so a component was swallowed and silently lost;
/// 2. a unit of more than two words — real units reach two (`fl oz`), and
///    swallowed prose runs longer.
///
/// Timers need no arm: `~{10 @salt}` is a hard cooklang parse error ("Timer
/// value is text"), so [`parse_recipe`] already refuses it.
///
/// Only (2) is a bound rather than a proof: a two-word swallow such as
/// `{200%g with}` still passes, because over-refusing a real recipe is worse
/// than that narrow miss. It is pinned by a test so the limit stays visible.
/// (1) has no such gap — any swallowed component carries its sigil with it.
fn reject_swallowed_components(recipe: &Recipe) -> Result<()> {
    let ingredients = recipe
        .ingredients
        .iter()
        .map(|i| ("ingredient", i.name.as_str(), i.quantity.as_ref()));
    let cookware = recipe
        .cookware
        .iter()
        .map(|c| ("cookware", c.name.as_str(), c.quantity.as_ref()));

    for (kind, name, quantity) in ingredients.chain(cookware) {
        let Some(quantity) = quantity else { continue };

        // A text amount is legitimate (`@salt{a pinch}`) — a SIGIL inside one
        // is not.
        if let CookValue::Text(text) = quantity.value() {
            if text.contains(['@', '#', '~']) {
                anyhow::bail!(
                    "cooklang: {kind} {name:?} has amount {text:?}, which contains a component \
                     sigil — its closing brace landed too late and swallowed the following \
                     component, which is now missing from the recipe entirely"
                );
            }
        }

        let Some(unit) = quantity.unit() else {
            continue;
        };
        if unit.contains(['@', '#', '~']) {
            anyhow::bail!(
                "cooklang: {kind} {name:?} has unit {unit:?}, which contains a component sigil — \
                 its closing brace landed too late and swallowed the following component, which \
                 is now missing from the recipe entirely"
            );
        }
        if unit.split_whitespace().count() > 2 {
            anyhow::bail!(
                "cooklang: {kind} {name:?} has unit {unit:?}, which is too many words to be a \
                 unit — its closing brace landed too late and swallowed following prose"
            );
        }
    }
    Ok(())
}

/// Every ingredient use in `source`, in first-reference order.
pub fn ingredient_uses(source: &str) -> Result<Vec<IngredientUse>> {
    let recipe = parse_recipe(source)?;
    uses_of(&recipe)
}

pub(crate) fn uses_of(recipe: &Recipe) -> Result<Vec<IngredientUse>> {
    let first_step = first_reference_steps(recipe);
    recipe
        .ingredients
        .iter()
        .enumerate()
        .map(|(idx, ing)| {
            let (quantity, unit) = match &ing.quantity {
                Some(q) => (numeric_value(q), q.unit().map(str::to_string)),
                None => (None, None),
            };
            // cooklang builds `ingredients` FROM step references, so every
            // entry has a referencing step. An absent one means that
            // invariant broke; `step_index` is 1-based and non-nullable, so
            // there is no in-band value to write — say so instead.
            let step_index = first_step.get(&idx).copied().ok_or_else(|| {
                anyhow::anyhow!(
                    "cooklang: ingredient {:?} is referenced by no step — cannot assign the \
                     1-based step_index its column requires",
                    ing.name
                )
            })?;
            Ok(IngredientUse {
                name: ing.name.clone(),
                quantity,
                unit,
                product_id: None,
                step_index,
            })
        })
        .collect()
}

/// Map ingredient index → the number of the first step referencing it.
fn first_reference_steps(recipe: &Recipe) -> std::collections::HashMap<usize, u32> {
    let mut out = std::collections::HashMap::new();
    for section in &recipe.sections {
        for content in &section.content {
            if let Content::Step(step) = content {
                for item in &step.items {
                    if let Item::Ingredient { index } = item {
                        out.entry(*index).or_insert(step.number);
                    }
                }
            }
        }
    }
    out
}

/// The numeric amount, when there is one. A range or a textual amount has no
/// single number and yields `None` rather than a fabricated midpoint.
fn numeric_value(q: &Quantity) -> Option<f64> {
    match q.value() {
        CookValue::Number(n) => Some(number_to_f64(n)),
        CookValue::Range { .. } | CookValue::Text(_) => None,
    }
}

fn number_to_f64(n: &Number) -> f64 {
    match n {
        Number::Regular(v) => *v,
        Number::Fraction {
            whole, num, den, ..
        } => *whole as f64 + (*num as f64 / *den as f64),
    }
}

/// Render one step's items back to plain prose: quantities inline, cooklang
/// sigils gone. This is what the reader sees on a step card.
pub(crate) fn step_text(recipe: &Recipe, items: &[Item]) -> String {
    let mut out = String::new();
    for item in items {
        match item {
            Item::Text { value } => out.push_str(value),
            Item::Ingredient { index } => {
                out.push_str(&recipe.ingredients[*index].display_name());
            }
            Item::Cookware { index } => out.push_str(&recipe.cookware[*index].name),
            Item::Timer { index } => {
                let timer = &recipe.timers[*index];
                match &timer.quantity {
                    Some(q) => out.push_str(&quantity_text(q)),
                    None => out.push_str(timer.name.as_deref().unwrap_or_default()),
                }
            }
            Item::InlineQuantity { index } => {
                out.push_str(&quantity_text(&recipe.inline_quantities[*index]));
            }
        }
    }
    out.trim().to_string()
}

fn quantity_text(q: &Quantity) -> String {
    let value = match q.value() {
        CookValue::Number(n) => format_number(number_to_f64(n)),
        CookValue::Range { start, end } => {
            format!(
                "{}-{}",
                format_number(number_to_f64(start)),
                format_number(number_to_f64(end))
            )
        }
        CookValue::Text(t) => t.clone(),
    };
    match q.unit() {
        Some(u) => format!("{value} {u}"),
        None => value,
    }
}

fn format_number(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}
