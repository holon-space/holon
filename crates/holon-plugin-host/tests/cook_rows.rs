//! A recipe's servings, sections, cookware and timers reach the store as typed
//! rows, and every value the plugin emits is a scalar: JSON text in a property
//! breaks boot.

use std::path::PathBuf;
use std::sync::LazyLock;

use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_plugin_host::PluginFormatAdapter;
use proptest::prelude::*;

use crate::support;

static PLUGIN: LazyLock<PluginFormatAdapter> = LazyLock::new(support::bundled_cook_plugin);

fn parse_fixture(name: &str) -> FileFormatParseResult {
    let root = support::fixtures_dir();
    let path = root.join(name);
    let content = std::fs::read_to_string(&path).unwrap();
    PLUGIN
        .parse(&path, &content, &EntityUri::no_parent(), &root)
        .unwrap_or_else(|e| panic!("{name} must ingest: {e:#}"))
}

fn parse_text(content: &str) -> Result<FileFormatParseResult, String> {
    let root = PathBuf::from("/vault");
    PLUGIN
        .parse(
            &root.join("Rezepte/T.cook"),
            content,
            &EntityUri::no_parent(),
            &root,
        )
        .map_err(|e| format!("{e:#}"))
}

fn rows<'a>(r: &'a FileFormatParseResult, type_name: &str) -> &'a [StorageEntity] {
    &r.typed_rows
        .iter()
        .find(|set| set.type_name == type_name)
        .unwrap_or_else(|| panic!("the parse projects no {type_name:?} scope"))
        .rows
}

fn row<'a>(rows: &'a [StorageEntity], id: &str) -> &'a StorageEntity {
    rows.iter()
        .find(|row| row.get("id") == Some(&s(id)))
        .unwrap_or_else(|| panic!("no row with id {id:?} in {rows:?}"))
}

fn cell(row: &StorageEntity, column: &str) -> Value {
    row.get(column)
        .cloned()
        .unwrap_or_else(|| panic!("row states no {column:?}: {row:?}"))
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

const DOUGH: &str = "recipe-section:brunch.cook::sec::0";
const TOPPING: &str = "recipe-section:brunch.cook::sec::1";

#[test]
fn servings_reach_the_recipe_row_as_a_number() {
    let brunch = parse_fixture("brunch.cook");
    assert_eq!(
        cell(&rows(&brunch, "recipe")[0], "servings"),
        Value::Integer(2)
    );
}

#[test]
fn servings_that_are_no_single_number_stay_empty_never_zero() {
    let r = parse_text("---\nservings: 4|6\n---\n\nBoil @water{1%l}.\n").unwrap();
    assert_eq!(cell(&rows(&r, "recipe")[0], "servings"), Value::Null);
    assert_eq!(
        r.document.get_property_str("servings"),
        Some("4|6".to_string()),
        "the text the cook wrote still reaches the page"
    );
}

#[test]
fn named_sections_project_in_source_order() {
    let brunch = parse_fixture("brunch.cook");
    let sections = rows(&brunch, "recipe_section");
    assert_eq!(sections.len(), 2, "{sections:?}");
    for (index, id, name) in [(0, DOUGH, "Dough"), (1, TOPPING, "Topping")] {
        let section = row(sections, id);
        assert_eq!(cell(section, "name"), s(name));
        assert_eq!(cell(section, "section_index"), Value::Integer(index));
        assert_eq!(cell(section, "recipe_id"), s("recipe:brunch.cook"));
    }
}

#[test]
fn ingredient_uses_name_their_section() {
    let brunch = parse_fixture("brunch.cook");
    let uses = rows(&brunch, "ingredient_use");
    for (part, section, step) in [
        ("flour-0", DOUGH, 1),
        ("water-0", DOUGH, 1),
        ("hollandaise-0", TOPPING, 1),
        ("onion-0", TOPPING, 1),
    ] {
        let u = row(uses, &format!("ingredient-use:brunch.cook::iu::{part}"));
        assert_eq!(cell(u, "section_id"), s(section), "{part}");
        assert_eq!(cell(u, "step_index"), Value::Integer(step), "{part}");
    }
}

#[test]
fn cookware_projects_as_rows() {
    let brunch = parse_fixture("brunch.cook");
    let cookware = rows(&brunch, "cookware");
    assert_eq!(cookware.len(), 2, "{cookware:?}");
    let bowl = row(cookware, "cookware:brunch.cook::cw::mixing-bowl-0");
    assert_eq!(cell(bowl, "name"), s("mixing bowl"));
    assert_eq!(cell(bowl, "quantity"), Value::Null);
    assert_eq!(cell(bowl, "recipe_id"), s("recipe:brunch.cook"));
    assert_eq!(cell(bowl, "section_id"), s(DOUGH));
    assert_eq!(cell(bowl, "step_index"), Value::Integer(1));
    let pan = row(cookware, "cookware:brunch.cook::cw::pan-0");
    assert_eq!(cell(pan, "quantity"), Value::Float(2.0));
    assert_eq!(cell(pan, "section_id"), s(TOPPING));
}

/// A timer keeps an id of its own — from its name, or its duration when
/// unnamed — and its duration in seconds; a range has no single duration.
#[test]
fn timers_project_as_rows_with_their_duration_in_seconds() {
    let brunch = parse_fixture("brunch.cook");
    let timers = rows(&brunch, "recipe_timer");
    assert_eq!(timers.len(), 3, "{timers:?}");
    #[rustfmt::skip]
    let expected = [
        // id part,          name,              amount,  seconds,              section, step
        ("10-minutes-0",     Value::Null,       "10",    Value::Float(600.0),  DOUGH,   2),
        ("rest-0",           s("rest"),         "1",     Value::Float(3600.0), DOUGH,   2),
        ("20-25-minutes-0",  Value::Null,       "20-25", Value::Null,          TOPPING, 2),
    ];
    for (part, name, amount, seconds, section, step) in expected {
        let t = row(timers, &format!("recipe-timer:brunch.cook::tm::{part}"));
        assert_eq!(cell(t, "name"), name, "{part}");
        assert_eq!(cell(t, "amount"), s(amount), "{part}");
        assert_eq!(cell(t, "seconds"), seconds, "{part}");
        assert_eq!(cell(t, "section_id"), s(section), "{part}");
        assert_eq!(cell(t, "step_index"), Value::Integer(step), "{part}");
        assert_eq!(cell(t, "recipe_id"), s("recipe:brunch.cook"));
    }
}

#[test]
fn a_german_timer_unit_converts_to_seconds() {
    let r = parse_fixture("kartoffelsuppe.cook");
    let seconds: Vec<Value> = rows(&r, "recipe_timer")
        .iter()
        .map(|t| cell(t, "seconds"))
        .collect();
    assert_eq!(seconds, vec![Value::Float(1200.0), Value::Float(3600.0)]);
}

#[test]
fn a_timer_without_a_unit_keeps_its_amount_and_no_seconds() {
    let r = parse_text("Wait ~{5}.\n").unwrap();
    let [timer] = rows(&r, "recipe_timer") else {
        panic!("one timer expected");
    };
    assert_eq!(cell(timer, "amount"), s("5"));
    assert_eq!(cell(timer, "unit"), Value::Null);
    assert_eq!(cell(timer, "seconds"), Value::Null);
}

#[test]
fn a_recipe_without_servings_sections_cookware_or_timers_projects_none() {
    let plain = parse_fixture("plain.cook");
    assert_eq!(cell(&rows(&plain, "recipe")[0], "servings"), Value::Null);
    for type_name in ["recipe_section", "cookware", "recipe_timer"] {
        assert!(rows(&plain, type_name).is_empty(), "{type_name}");
    }
    let butter = row(
        rows(&plain, "ingredient_use"),
        "ingredient-use:plain.cook::iu::butter-0",
    );
    assert_eq!(cell(butter, "section_id"), Value::Null);
}

/// Cooklang's picture convention: `<stem>.jpg` beside the file is the
/// recipe's, `<stem>.<n>.jpg` the n-th step's (1-based, counted over the whole
/// recipe so a step in a later section has one number).
#[test]
fn sibling_images_reach_the_recipe_and_its_step() {
    let brunch = parse_fixture("brunch.cook");
    assert_eq!(cell(&rows(&brunch, "recipe")[0], "image"), s("brunch.jpg"));
    let images: Vec<(String, Option<String>)> = brunch
        .blocks
        .iter()
        .map(|b| (b.content.clone(), b.get_property_str("image")))
        .collect();
    let spread = images
        .iter()
        .find(|(content, _)| content.starts_with("Spread"))
        .unwrap();
    assert_eq!(spread.1.as_deref(), Some("brunch.3.jpg"));
    assert_eq!(
        images.iter().filter(|(_, image)| image.is_some()).count(),
        1,
        "{images:?}"
    );

    let pancakes = parse_fixture("pancakes.cook");
    assert_eq!(cell(&rows(&pancakes, "recipe")[0], "image"), Value::Null);
}

#[test]
fn a_picture_added_after_a_parse_shows_on_the_next_parse() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("cook-rows-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("plain.cook");
    let content = std::fs::read_to_string(support::fixtures_dir().join("plain.cook")).unwrap();
    std::fs::write(&path, &content).unwrap();
    let plugin = support::bundled_cook_plugin();
    let image = |plugin: &PluginFormatAdapter| {
        let parsed = plugin
            .parse(&path, &content, &EntityUri::no_parent(), &dir)
            .unwrap();
        cell(&rows(&parsed, "recipe")[0], "image")
    };

    assert_eq!(image(&plugin), Value::Null);
    std::fs::write(dir.join("plain.png"), b"").unwrap();
    assert_eq!(image(&plugin), s("plain.png"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A string that parses as a JSON object or array is structure smuggled into a
/// scalar; the store must never see one.
fn assert_no_json_text(r: &FileFormatParseResult, origin: &str) {
    let is_json_text = |value: &Value| match value {
        Value::String(text) => matches!(
            serde_json::from_str::<serde_json::Value>(text),
            Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_))
        ),
        _ => false,
    };
    for block in std::iter::once(&r.document).chain(&r.blocks) {
        for (key, value) in &block.properties {
            assert!(
                !is_json_text(value),
                "{origin}: block {:?} property {key} is JSON text: {value:?}",
                block.content
            );
        }
    }
    for set in &r.typed_rows {
        for row in &set.rows {
            for (column, value) in row {
                assert!(
                    !is_json_text(value),
                    "{origin}: {} column {column} is JSON text: {value:?}",
                    set.type_name
                );
            }
        }
    }
}

#[test]
fn no_emitted_value_is_json_text() {
    for fixture in [
        "brunch.cook",
        "pancakes.cook",
        "kartoffelsuppe.cook",
        "plain.cook",
    ] {
        assert_no_json_text(&parse_fixture(fixture), fixture);
    }
}

/// Timer amounts with no duration in them: cooklang refuses the recipe.
const REFUSED_TIMER_AMOUNTS: [&str; 3] = ["a while", "3%g", ""];

fn timer_amount() -> impl Strategy<Value = String> {
    prop_oneof![
        (0u32..200).prop_map(|n| n.to_string()),
        (0u32..200).prop_map(|n| format!("{n}%minutes")),
        (0u32..200).prop_map(|n| format!("{n}%Minuten")),
        (0u32..200).prop_map(|n| format!("{n}%h")),
        (1u32..9, 10u32..20).prop_map(|(a, b)| format!("{a}-{b}%min")),
        (1u32..9).prop_map(|n| format!("1/{n}%hour")),
        Just("a while".to_string()),
        Just("3%g".to_string()),
        Just(String::new()),
    ]
}

fn component_amount() -> impl Strategy<Value = Option<String>> {
    proptest::option::of(prop_oneof![
        Just(String::new()),
        (0u32..9).prop_map(|n| n.to_string()),
        (1u32..9).prop_map(|n| format!("{n}%l")),
        Just("a few".to_string()),
        (1u32..4, 4u32..9).prop_map(|(a, b)| format!("{a}-{b}")),
    ])
}

#[derive(Debug, Clone)]
enum Piece {
    Text(&'static str),
    Cookware(&'static str, Option<String>),
    Timer(Option<&'static str>, String),
}

fn piece() -> impl Strategy<Value = Piece> {
    let names = || proptest::sample::select(vec!["pan", "big pot", "Öfen", "pan-2", "x"]);
    prop_oneof![
        proptest::sample::select(vec!["stir ", "then ", "für "]).prop_map(Piece::Text),
        (names(), component_amount()).prop_map(|(n, a)| Piece::Cookware(n, a)),
        (proptest::option::of(names()), timer_amount()).prop_map(|(n, a)| Piece::Timer(n, a)),
    ]
}

fn render_piece(piece: &Piece) -> String {
    match piece {
        Piece::Text(text) => text.to_string(),
        Piece::Cookware(name, None) if !name.contains(' ') => format!("#{name} "),
        Piece::Cookware(name, amount) => {
            format!("#{name}{{{}}} ", amount.as_deref().unwrap_or(""))
        }
        Piece::Timer(name, amount) => format!("~{}{{{amount}}} ", name.unwrap_or("")),
    }
}

#[derive(Debug, Clone)]
struct Section {
    name: Option<&'static str>,
    steps: Vec<Vec<Piece>>,
}

fn recipe() -> impl Strategy<Value = (Option<String>, Vec<Section>)> {
    let section = (
        proptest::option::of(proptest::sample::select(vec!["Dough", "Für die Soße", ""])),
        proptest::collection::vec(proptest::collection::vec(piece(), 0..5), 0..4),
    )
        .prop_map(|(name, steps)| Section { name, steps });
    (
        proptest::option::of(prop_oneof![
            (0u32..20).prop_map(|n| n.to_string()),
            Just("4|6".to_string()),
            Just("two".to_string()),
            Just("-1".to_string()),
            Just("99999999999".to_string()),
        ]),
        proptest::collection::vec(section, 0..4),
    )
}

fn render_recipe(servings: &Option<String>, sections: &[Section]) -> String {
    let mut out = String::new();
    if let Some(servings) = servings {
        out.push_str(&format!("---\nservings: {servings}\n---\n\n"));
    }
    for section in sections {
        if let Some(name) = section.name {
            out.push_str(&format!("== {name} ==\n\n"));
        }
        for step in &section.steps {
            out.push_str("Go ");
            for piece in step {
                out.push_str(&render_piece(piece));
            }
            out.push_str("\n\n");
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Every component the cook wrote is one row, every section a row names is
    /// one the recipe projects, and nothing emitted is JSON text.
    #[test]
    fn every_component_becomes_one_row(
        (servings, sections) in recipe(),
    ) {
        let text = render_recipe(&servings, &sections);
        let refused_timer = sections
            .iter()
            .flat_map(|s| s.steps.iter().flatten())
            .any(|p| matches!(p, Piece::Timer(_, amount) if REFUSED_TIMER_AMOUNTS.contains(&amount.as_str())));
        let parsed = match parse_text(&text) {
            Ok(parsed) => parsed,
            Err(refusal) => {
                prop_assert!(
                    refused_timer && refusal.contains("cooklang"),
                    "only a timer that is no duration is refused, by name: {refusal}\n--- recipe ---\n{text}"
                );
                return Ok(());
            }
        };
        prop_assert!(!refused_timer, "a timer that is no duration was accepted\n{}", text);
        assert_no_json_text(&parsed, &text);

        let pieces = sections.iter().flat_map(|s| s.steps.iter().flatten());
        let cookware = pieces.clone().filter(|p| matches!(p, Piece::Cookware(..))).count();
        let timers = pieces.filter(|p| matches!(p, Piece::Timer(..))).count();
        prop_assert_eq!(rows(&parsed, "cookware").len(), cookware, "{}", text);
        prop_assert_eq!(rows(&parsed, "recipe_timer").len(), timers, "{}", text);

        let section_ids: Vec<Value> = rows(&parsed, "recipe_section")
            .iter()
            .map(|section| cell(section, "id"))
            .collect();
        for type_name in ["ingredient_use", "cookware", "recipe_timer"] {
            for component in rows(&parsed, type_name) {
                let section = cell(component, "section_id");
                prop_assert!(
                    section == Value::Null || section_ids.contains(&section),
                    "{} names section {:?} the recipe does not project\n{}",
                    type_name, section, text
                );
            }
        }
        let mut ids: Vec<Value> = parsed
            .typed_rows
            .iter()
            .flat_map(|set| set.rows.iter().map(|row| cell(row, "id")))
            .collect();
        let total = ids.len();
        ids.sort_by_key(|id| format!("{id:?}"));
        ids.dedup();
        prop_assert_eq!(ids.len(), total, "two rows share an id\n{}", text);
    }
}
