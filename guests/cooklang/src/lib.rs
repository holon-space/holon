//! The `.cook` format as a wasm guest: cooklang 0.18.7 behind the five-function
//! ABI, emitting the plugin row stream of `holon-plugin-rows`.
//!
//! A pure function. It imports nothing — no WASI, no clock, no filesystem — so
//! the only thing that reaches it is the file's bytes and a JSON context
//! naming the vault-relative `source_path`, the file's `file_stem` and the
//! `sibling_files` beside it.
//!
//! Every refusal here is the whole file's refusal: the host emits no scope and
//! no document block for a stream it never receives.

use std::collections::HashMap;

use cooklang::CooklangParser;
use cooklang::Extensions;
use cooklang::Item;
use cooklang::Recipe;
use cooklang::convert::Converter;
use cooklang::convert::PhysicalQuantity;
use cooklang::convert::UnitsFile;
use cooklang::model::Content;
use cooklang::quantity::Number;
use cooklang::quantity::Quantity;
use cooklang::quantity::Value as CookValue;
use holon_plugin_rows::BlockKey;
use holon_plugin_rows::BlockRow;
use holon_plugin_rows::DocumentRow;
use holon_plugin_rows::EntityRef;
use holon_plugin_rows::Fields;
use holon_plugin_rows::Line;
use holon_plugin_rows::LocalId;
use holon_plugin_rows::Owner;
use holon_plugin_rows::Row;
use holon_plugin_rows::Scope;
use holon_plugin_rows::Stream;
use serde_json::Value;
use serde_json::json;

holon_abi_guest::holon_plugin!(parse_cook);

/// The default dlmalloc coalesces and splits on nearly every free, which costs
/// about twice the fuel per small allocation; the parse allocates per token.
#[global_allocator]
static ALLOC: talc::TalckWasm = unsafe { talc::TalckWasm::new_global() };

/// The property carrying a step's 1-based number, which distinguishes a step
/// block from a prose block.
const STEP_NUMBER_KEY: &str = "step_number";

/// The property carrying the vault-relative path of a step's picture.
const IMAGE_KEY: &str = "image";

/// cooklang's picture convention: `<stem>.jpg` beside the recipe is its own,
/// `<stem>.<n>.jpg` the picture of its n-th step.
const IMAGE_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];

/// The types whose rows the recipe owns through `recipe_id`.
const OWNED_TYPES: [&str; 4] = [
    "ingredient_use",
    "recipe_section",
    "cookware",
    "recipe_timer",
];

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

    let recipe_id = LocalId::from_path(source_path).map_err(|e| e.to_string())?;
    let recipe_ref = EntityRef {
        type_name: "recipe".to_string(),
        id: recipe_id.clone(),
    };
    let mut scopes = vec![Scope {
        type_name: "recipe".to_string(),
        owner_column: "source_path".to_string(),
        owner: Owner::Text(source_path.to_string()),
    }];
    for type_name in OWNED_TYPES {
        scopes.push(Scope {
            type_name: type_name.to_string(),
            owner_column: "recipe_id".to_string(),
            owner: Owner::Ref(recipe_ref.clone()),
        });
    }

    let siblings = sibling_files(&ctx)?;
    let dir = source_path
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();
    let image_of = |base: &str| {
        IMAGE_EXTENSIONS
            .iter()
            .map(|ext| format!("{base}.{ext}"))
            .find(|name| siblings.contains(&name.as_str()))
            .map(|name| format!("{dir}{name}"))
    };

    let mut lines = Vec::new();
    let mut document = Fields::new();
    for (key, value) in properties {
        document.insert(key, Value::String(value));
    }
    lines.push(Line::Document(DocumentRow {
        title: title.clone(),
        properties: document,
    }));

    let mut step_ordinal = 0;
    for (seq, block) in blocks_of(&recipe).into_iter().enumerate() {
        let mut properties = Fields::new();
        if let Some(number) = block.step_number {
            properties.insert(STEP_NUMBER_KEY, json!(number.to_string()));
            step_ordinal += 1;
            if let Some(image) = image_of(&format!("{file_stem}.{step_ordinal}")) {
                properties.insert(IMAGE_KEY, json!(image));
            }
        }
        lines.push(Line::Block(BlockRow {
            key: BlockKey::new(["b".to_string(), seq.to_string()]).map_err(|e| e.to_string())?,
            content: block.text,
            properties,
        }));
    }

    let mut cells = Fields::new();
    cells.insert("source_path", json!(source_path));
    cells.insert("title", json!(title));
    cells.insert("course", json!(course));
    // A servings text such as `4|6` has no single number; the page still
    // shows it through the document's properties.
    cells.insert(
        "servings",
        json!(recipe.metadata.servings().and_then(|s| s.as_number())),
    );
    cells.insert("image", json!(image_of(file_stem)));
    lines.push(Line::Row(Row {
        type_name: "recipe".to_string(),
        id: recipe_id.clone(),
        refs: Fields::new(),
        cells,
    }));

    let owned_row = |type_name: &str, id: LocalId, section: usize, cells: Fields<Value>| {
        let mut refs = Fields::new();
        refs.insert("recipe_id", recipe_ref.clone());
        let mut cells = cells;
        match section_ref(&recipe, &recipe_id, section)? {
            Some(section) => refs.insert("section_id", section),
            None => cells.insert("section_id", Value::Null),
        }
        Ok::<_, String>(Line::Row(Row {
            type_name: type_name.to_string(),
            id,
            refs,
            cells,
        }))
    };

    for (index, section) in recipe.sections.iter().enumerate() {
        let Some(name) = &section.name else { continue };
        let mut refs = Fields::new();
        refs.insert("recipe_id", recipe_ref.clone());
        let mut cells = Fields::new();
        cells.insert("name", json!(name));
        cells.insert("section_index", json!(index));
        lines.push(Line::Row(Row {
            type_name: "recipe_section".to_string(),
            id: component_id(&recipe_id, "sec", index.to_string())?,
            refs,
            cells,
        }));
    }

    let placements = placements_of(&recipe);

    let mut seen: HashMap<String, usize> = HashMap::new();
    for (index, ingredient) in recipe.ingredients.iter().enumerate() {
        let at = placement(
            &placements.ingredients,
            index,
            "ingredient",
            &ingredient.name,
        )?;
        let (quantity, unit) = match &ingredient.quantity {
            Some(q) => (numeric_value(q), q.unit().map(str::to_string)),
            None => (None, None),
        };
        let mut cells = Fields::new();
        // The schema's column for what the parser calls `name`.
        cells.insert("raw_name", json!(ingredient.name));
        cells.insert("quantity", json!(quantity));
        cells.insert("unit", json!(unit));
        cells.insert("step_index", json!(at.step));
        let id = component_id(
            &recipe_id,
            "iu",
            occurrence_part(&mut seen, &ingredient.name),
        )?;
        lines.push(owned_row("ingredient_use", id, at.section, cells)?);
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    for (index, cookware) in recipe.cookware.iter().enumerate() {
        let at = placement(&placements.cookware, index, "cookware", &cookware.name)?;
        let mut cells = Fields::new();
        cells.insert("name", json!(cookware.name));
        cells.insert(
            "quantity",
            json!(cookware.quantity.as_ref().and_then(numeric_value)),
        );
        cells.insert("step_index", json!(at.step));
        let id = component_id(&recipe_id, "cw", occurrence_part(&mut seen, &cookware.name))?;
        lines.push(owned_row("cookware", id, at.section, cells)?);
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    for (index, timer) in recipe.timers.iter().enumerate() {
        let quantity = timer.quantity.as_ref().ok_or_else(|| {
            format!(
                "cooklang: timer {:?} has no duration, which TIMER_REQUIRES_TIME refuses",
                timer.name
            )
        })?;
        let label = timer.name.clone().unwrap_or_else(|| quantity.to_string());
        let at = placement(&placements.timers, index, "timer", &label)?;
        let mut cells = Fields::new();
        cells.insert("name", json!(timer.name));
        cells.insert("amount", json!(quantity.value().to_string()));
        cells.insert("unit", json!(quantity.unit()));
        cells.insert("seconds", json!(timer_seconds(quantity)?));
        cells.insert("step_index", json!(at.step));
        let id = component_id(&recipe_id, "tm", occurrence_part(&mut seen, &label))?;
        lines.push(owned_row("recipe_timer", id, at.section, cells)?);
    }

    Ok(Stream { scopes, lines }.to_jsonl())
}

fn cell<'a>(ctx: &'a Value, key: &str) -> Result<&'a str, String> {
    ctx[key]
        .as_str()
        .ok_or_else(|| format!("context is missing string `{key}`"))
}

struct BlockText {
    text: String,
    /// `Some` for a step, `None` for prose — the distinction the recipe page
    /// renders on.
    step_number: Option<u32>,
}

/// The German unit vocabulary, layered over cooklang's bundled English units.
///
/// The guest has no filesystem, so the units file is compiled in rather than
/// read. Without it a timer written `~{20%Minuten}` is an ERROR and costs the
/// reader the whole recipe (ruling D100.a).
///
/// It lives under `src/` because `guests/build.sh` keys its restaging on that
/// directory: a copy anywhere else would edit without triggering a rebuild.
const GERMAN_UNITS: &str = include_str!("german.toml");

/// A parser matching `cooklang::parse` — every extension, the bundled units —
/// plus the German layer.
fn parser() -> Result<CooklangParser, String> {
    let german: UnitsFile = toml::from_str(GERMAN_UNITS)
        .map_err(|e| format!("the compiled-in German units file is not valid: {e}"))?;
    let converter = Converter::builder()
        .with_bundled_units()
        .and_then(|b| b.with_units_file(german))
        .and_then(|b| b.finish())
        .map_err(|e| {
            format!("the German units layer does not build onto the bundled units: {e}")
        })?;
    Ok(CooklangParser::new(Extensions::all(), converter))
}

thread_local! {
    /// Building the unit tables costs more fuel than parsing a typical recipe,
    /// and the host reuses one instance for every file of a scan.
    static PARSER: Result<CooklangParser, String> = parser();
}

fn parse_recipe(source: &str) -> Result<Recipe, String> {
    reject_unclosed_component_brace(source)?;
    let parsed = PARSER.with(|parser| {
        parser
            .as_ref()
            .map(|p| p.parse(source))
            .map_err(Clone::clone)
    })?;
    let recipe = parsed
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

/// Where a step placed a component: the index of its section and the step's
/// number, which restarts at 1 in every section.
#[derive(Clone, Copy)]
struct Placement {
    section: usize,
    step: u32,
}

#[derive(Default)]
struct Placements {
    ingredients: HashMap<usize, Placement>,
    cookware: HashMap<usize, Placement>,
    timers: HashMap<usize, Placement>,
}

fn placements_of(recipe: &Recipe) -> Placements {
    let mut out = Placements::default();
    for (section, body) in recipe.sections.iter().enumerate() {
        for content in &body.content {
            let Content::Step(step) = content else {
                continue;
            };
            let at = Placement {
                section,
                step: step.number,
            };
            for item in &step.items {
                let (map, index) = match item {
                    Item::Ingredient { index } => (&mut out.ingredients, index),
                    Item::Cookware { index } => (&mut out.cookware, index),
                    Item::Timer { index } => (&mut out.timers, index),
                    Item::Text { .. } | Item::InlineQuantity { .. } => continue,
                };
                map.entry(*index).or_insert(at);
            }
        }
    }
    out
}

/// cooklang builds its component lists FROM step references, so every entry
/// has a step; the non-nullable `step_index` has no value for one without.
fn placement(
    placements: &HashMap<usize, Placement>,
    index: usize,
    kind: &str,
    name: &str,
) -> Result<Placement, String> {
    placements.get(&index).copied().ok_or_else(|| {
        format!("cooklang: {kind} {name:?} is referenced by no step — it has no step to sit in")
    })
}

/// The row of a named section; an unnamed one (the part before the first
/// `== Name ==`) projects none.
fn section_ref(
    recipe: &Recipe,
    recipe_id: &LocalId,
    section: usize,
) -> Result<Option<EntityRef>, String> {
    if recipe.sections[section].name.is_none() {
        return Ok(None);
    }
    Ok(Some(EntityRef {
        type_name: "recipe_section".to_string(),
        id: component_id(recipe_id, "sec", section.to_string())?,
    }))
}

fn component_id(recipe_id: &LocalId, kind: &str, part: String) -> Result<LocalId, String> {
    recipe_id
        .clone()
        .part(kind)
        .and_then(|id| id.part(part))
        .map_err(|e| e.to_string())
}

/// `<slug>-<n>`, `n` counting earlier components of the same slug — counted on
/// the slug, not the name, because distinct names may share one.
fn occurrence_part(seen: &mut HashMap<String, usize>, name: &str) -> String {
    let slug = id_slug(name);
    let occurrence = seen.entry(slug.clone()).or_insert(0);
    let part = format!("{slug}-{occurrence}");
    *occurrence += 1;
    part
}

/// A timer's duration in seconds; `None` for a range, which has no single
/// duration, and for a bare number, which has no unit to scale.
fn timer_seconds(quantity: &Quantity) -> Result<Option<f64>, String> {
    if quantity.unit().is_none() {
        return Ok(None);
    }
    let ratio = PARSER.with(|parser| {
        let parser = parser.as_ref().map_err(Clone::clone)?;
        quantity
            .unit_info(parser.converter())
            .filter(|unit| unit.physical_quantity == PhysicalQuantity::Time)
            .map(|unit| unit.ratio)
            .ok_or_else(|| {
                format!("cooklang: timer {quantity} has no time unit, which ADVANCED_UNITS refuses")
            })
    })?;
    match quantity.value() {
        CookValue::Number(n) => Ok(Some(number_to_f64(n) * ratio)),
        CookValue::Range { .. } => Ok(None),
        CookValue::Text(text) => Err(format!(
            "cooklang: timer value {text:?} is text, which ADVANCED_UNITS refuses"
        )),
    }
}

/// The names of the files beside the recipe, as the host lists them.
fn sibling_files(ctx: &Value) -> Result<Vec<&str>, String> {
    ctx["sibling_files"]
        .as_array()
        .ok_or("context is missing the `sibling_files` list")?
        .iter()
        .map(|name| {
            name.as_str()
                .ok_or_else(|| format!("context `sibling_files` holds {name}, not a file name"))
        })
        .collect()
}

/// A component name reduced to lowercase ASCII, so `@Sea Salt` and
/// `@sea salt` share an id part.
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
