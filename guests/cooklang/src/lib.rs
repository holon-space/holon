//! The `.cook` format as a wasm guest: cooklang 0.18.7 behind the four-function
//! ABI, emitting the JSON Lines row contract.
//!
//! A pure function. It imports nothing — no WASI, no clock, no filesystem — so
//! the only thing that reaches it is the file's bytes and a JSON context
//! naming the vault-relative `source_path` and the file's `file_stem`.
//!
//! Every refusal here is the whole file's refusal: the host emits no scope and
//! no document block for a stream it never receives.

use std::collections::HashMap;

use cooklang::Item;
use cooklang::Recipe;
use cooklang::model::Content;
use cooklang::quantity::Number;
use cooklang::quantity::Quantity;
use cooklang::quantity::Value as CookValue;
use serde_json::Value;
use serde_json::json;

holon_abi_guest::holon_plugin!(parse_cook);

/// The property carrying a step's 1-based number, which distinguishes a step
/// block from a prose block.
const STEP_NUMBER_KEY: &str = "step_number";

fn parse_cook(input: &[u8], ctx: &[u8]) -> Result<String, String> {
    let source = core::str::from_utf8(input).map_err(|e| format!("input is not UTF-8: {e}"))?;
    let ctx: Value =
        serde_json::from_slice(ctx).map_err(|e| format!("context is not JSON: {e}"))?;
    let source_path = cell(&ctx, "source_path")?;
    let file_stem = cell(&ctx, "file_stem")?;

    let recipe = parse_recipe(source)?;

    // No placeholder title: a name we invented would look like the recipe's
    // own and quietly become its identity.
    let title = recipe
        .metadata
        .title()
        .map(str::to_string)
        .unwrap_or_else(|| file_stem.to_string());

    // Every metadata key except the title becomes a document property.
    // Nothing is skipped quietly: an unrepresentable key or value is refused
    // by name, because a recipe's `tags:` disappearing without a word is the
    // silent-degradation outcome the error ladder forbids.
    let mut properties: Vec<(String, String)> = Vec::new();
    for (key, value) in recipe.metadata.map.iter() {
        let key = key.as_str().ok_or_else(|| {
            format!("cooklang metadata key {key:?} is not a string and cannot name a property")
        })?;
        if key.eq_ignore_ascii_case("title") {
            continue;
        }
        properties.push((key.to_string(), metadata_value_text(key, value)?));
    }
    let course = properties
        .iter()
        .find(|(key, _)| key == "course")
        .map(|(_, value)| value.clone());

    let recipe_id = format!("recipe:{source_path}");
    let mut out = String::new();

    push(
        &mut out,
        json!({
            "holon_rows": 1,
            "scopes": [
                {"type": "holon.document", "owner_column": "source_path", "owner_value": source_path},
                {"type": "holon.block", "owner_column": "source_path", "owner_value": source_path},
                {"type": "recipe", "owner_column": "source_path", "owner_value": source_path},
                {"type": "ingredient_use", "owner_column": "recipe_id", "owner_value": recipe_id},
            ]
        }),
    );

    let mut document = serde_json::Map::new();
    document.insert("title".to_string(), Value::String(title.clone()));
    for (key, value) in properties {
        document.insert(key, Value::String(value));
    }
    push(
        &mut out,
        json!({"type": "holon.document", "row": Value::Object(document)}),
    );

    for (seq, block) in blocks_of(&recipe).into_iter().enumerate() {
        let mut row = serde_json::Map::new();
        row.insert("id".to_string(), json!(format!("b::{seq}")));
        row.insert("content".to_string(), Value::String(block.text));
        if let Some(number) = block.step_number {
            row.insert(STEP_NUMBER_KEY.to_string(), json!(number.to_string()));
        }
        push(
            &mut out,
            json!({"type": "holon.block", "row": Value::Object(row)}),
        );
    }

    push(
        &mut out,
        json!({
            "type": "recipe",
            "row": {
                "id": source_path,
                "source_path": source_path,
                "title": title,
                // `servings` is deliberately not written: cooklang admits
                // non-integer servings (`4|6|8`) that the INTEGER column
                // cannot hold, and the metadata reaches the recipe page
                // through the document block's properties either way.
                "course": course,
            }
        }),
    );

    let mut seen: HashMap<String, usize> = HashMap::new();
    for use_ in uses_of(&recipe)? {
        let slug = id_slug(&use_.name);
        let occurrence = seen.entry(slug.clone()).or_insert(0);
        let local = format!("{source_path}::iu::{slug}-{occurrence}");
        *occurrence += 1;
        push(
            &mut out,
            json!({
                "type": "ingredient_use",
                "row": {
                    "id": local,
                    "recipe_id": recipe_id,
                    // The schema's column for what the parser calls `name`.
                    "raw_name": use_.name,
                    "quantity": use_.quantity,
                    "unit": use_.unit,
                    "step_index": use_.step_index,
                }
            }),
        );
    }

    Ok(out)
}

fn push(out: &mut String, line: Value) {
    out.push_str(&line.to_string());
    out.push('\n');
}

fn cell<'a>(ctx: &'a Value, key: &str) -> Result<&'a str, String> {
    ctx[key]
        .as_str()
        .ok_or_else(|| format!("context is missing string `{key}`"))
}

/// One use of one ingredient by one recipe step.
struct IngredientUse {
    name: String,
    /// `None` for a bare ingredient (`@salt`) and for a non-numeric amount
    /// (`@salt{a pinch}`), which cooklang admits and no `REAL` column can hold.
    quantity: Option<f64>,
    unit: Option<String>,
    /// 1-based step number this ingredient is first referenced from.
    step_index: u32,
}

struct BlockText {
    text: String,
    /// `Some` for a step, `None` for prose — the distinction the recipe page
    /// renders on.
    step_number: Option<u32>,
}

fn parse_recipe(source: &str) -> Result<Recipe, String> {
    reject_unclosed_component_brace(source)?;
    let recipe = cooklang::parse(source)
        .into_result()
        .map(|(recipe, _warnings)| recipe)
        .map_err(|report| {
            format!(
                "cooklang source is not a valid recipe: cooklang parse failed: {}",
                // The report renders every diagnostic; keep them all rather
                // than surfacing only the first.
                report
                    .errors()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?;
    reject_swallowed_components(&recipe)?;
    Ok(recipe)
}

/// Refuse a component brace that never closes on its line.
///
/// cooklang accepts `@flour{200%g` and SILENTLY drops the quantity — no error,
/// no warning, an ingredient with no amount. Only an unclosed OPEN brace is
/// refused: a surplus `}` is ordinary prose (`#pot{} and a } sign`) and must
/// keep parsing.
fn reject_unclosed_component_brace(source: &str) -> Result<(), String> {
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
            return Err(format!(
                "cooklang line {}: a component brace is never closed — cooklang would silently \
                 drop the quantity here: {line:?}",
                n + 1
            ));
        }
    }
    Ok(())
}

/// Refuse a parsed component whose amount shows that its closing brace landed
/// too late and swallowed following text — `@flour{200%g @sugar}` parses
/// cleanly with unit `"g @sugar"` and `@sugar` gone entirely.
///
/// Two arms, over ingredients AND cookware: a component sigil in the unit or in
/// a text value, and a unit of more than two words (real units reach two, as in
/// `fl oz`). The second is a bound rather than a proof — a two-word swallow
/// still passes, because over-refusing a real recipe is worse than that narrow
/// miss.
fn reject_swallowed_components(recipe: &Recipe) -> Result<(), String> {
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
                return Err(format!(
                    "cooklang: {kind} {name:?} has amount {text:?}, which contains a component \
                     sigil — its closing brace landed too late and swallowed the following \
                     component, which is now missing from the recipe entirely"
                ));
            }
        }

        let Some(unit) = quantity.unit() else {
            continue;
        };
        if unit.contains(['@', '#', '~']) {
            return Err(format!(
                "cooklang: {kind} {name:?} has unit {unit:?}, which contains a component sigil — \
                 its closing brace landed too late and swallowed the following component, which \
                 is now missing from the recipe entirely"
            ));
        }
        if unit.split_whitespace().count() > 2 {
            return Err(format!(
                "cooklang: {kind} {name:?} has unit {unit:?}, which is too many words to be a \
                 unit — its closing brace landed too late and swallowed following prose"
            ));
        }
    }
    Ok(())
}

/// Render one recipe-metadata value as the text a property holds.
///
/// A sequence of scalars — standard cooklang `tags: [quick, vegan]` — joins to
/// `"quick, vegan"`. Anything else has no single-column form, and per fail-loud
/// policy is REFUSED by name rather than silently skipped.
fn metadata_value_text(key: &str, value: &serde_yaml::Value) -> Result<String, String> {
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
                    other => {
                        return Err(format!(
                            "cooklang metadata {key:?}: list entry {other:?} is not a scalar and \
                             has no single-column form"
                        ));
                    }
                }
            }
            Ok(parts.join(", "))
        }
        other => Err(format!(
            "cooklang metadata {key:?}: value {other:?} is neither a scalar nor a list of scalars \
             and has no single-column form"
        )),
    }
}

fn blocks_of(recipe: &Recipe) -> Vec<BlockText> {
    let mut out = Vec::new();
    for section in &recipe.sections {
        for content in &section.content {
            match content {
                Content::Step(step) => out.push(BlockText {
                    text: step_text(recipe, &step.items),
                    step_number: Some(step.number),
                }),
                Content::Text(text) => out.push(BlockText {
                    text: text.trim().to_string(),
                    step_number: None,
                }),
            }
        }
    }
    out
}

fn uses_of(recipe: &Recipe) -> Result<Vec<IngredientUse>, String> {
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
            // entry has a referencing step. An absent one means that invariant
            // broke; `step_index` is 1-based and non-nullable, so there is no
            // in-band value to write — say so instead.
            let step_index = first_step.get(&idx).copied().ok_or_else(|| {
                format!(
                    "cooklang: ingredient {:?} is referenced by no step — cannot assign the \
                     1-based step_index its column requires",
                    ing.name
                )
            })?;
            Ok(IngredientUse {
                name: ing.name.clone(),
                quantity,
                unit,
                step_index,
            })
        })
        .collect()
}

/// Map ingredient index → the number of the first step referencing it.
fn first_reference_steps(recipe: &Recipe) -> HashMap<usize, u32> {
    let mut out = HashMap::new();
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
fn step_text(recipe: &Recipe, items: &[Item]) -> String {
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
