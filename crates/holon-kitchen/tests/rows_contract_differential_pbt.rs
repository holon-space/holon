//! Differential: the rows `CookFormatAdapter` produces survive the neutral
//! JSON-Lines contract unchanged.
//!
//! The reference is the adapter's own `typed_rows`; the SUT is those rows put
//! through `holon_rows::emit_row_sets` → `parse_row_sets`. Inc 2 replaces the
//! adapter with a plugin speaking that contract, so anything the contract
//! cannot carry today is a row the plugin would silently lose.
//!
//! Only one `.cook` fixture exists, so the recipes come from a generator: the
//! interesting shapes — a recipe with NO ingredients (an empty
//! `ingredient_use` scope), a bare `@salt` (NULL quantity AND unit), two uses
//! whose names share a slug — are all rare in hand-written fixtures and all
//! load-bearing for the contract.

use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::TypedRowSet;
use holon_kitchen::CookFormatAdapter;
use holon_rows::emit_row_sets;
use holon_rows::parse_row_sets;
use proptest::prelude::*;

fn rows_of(rel: &str, content: &str) -> Vec<TypedRowSet> {
    let root = PathBuf::from("/vault");
    CookFormatAdapter::new()
        .parse(&root.join(rel), content, &EntityUri::no_parent(), &root)
        .expect("the generator only produces recipes the adapter accepts")
        .typed_rows
}

fn round_trip(sets: &[TypedRowSet]) -> Vec<TypedRowSet> {
    let text = emit_row_sets(sets).expect("adapter rows must be emittable");
    parse_row_sets(&text).expect("our own stream must parse")
}

/// Ingredient names the cooklang grammar reads as one name and whose slugs
/// exercise the id path: accents, a shared slug (`sea salt` / `sea-salt`), and
/// repeats that drive the occurrence counter.
const NAMES: &[&str] = &[
    "flour",
    "eggs",
    "sea salt",
    "sea-salt",
    "crème fraîche",
    "butter",
    "maple syrup",
];

#[derive(Debug, Clone)]
enum Amount {
    /// `@salt` — no braces at all.
    Bare,
    /// `@salt{}` — the spike's NULL case: an empty amount, never a zero.
    Empty,
    /// `@flour{200}`
    Number(u32),
    /// `@flour{200%g}`
    Numbered(u32, &'static str),
    /// `@salt{a pinch}` — cooklang admits it and no REAL column holds it.
    Text,
}

fn amount() -> impl Strategy<Value = Amount> {
    prop_oneof![
        Just(Amount::Bare),
        Just(Amount::Empty),
        (1u32..500).prop_map(Amount::Number),
        (1u32..500, prop_oneof![Just("g"), Just("ml"), Just("tsp")])
            .prop_map(|(n, u)| Amount::Numbered(n, u)),
        Just(Amount::Text),
    ]
}

fn render_ingredient(name: &str, amount: &Amount) -> String {
    match amount {
        Amount::Bare => format!("@{name}"),
        Amount::Empty => format!("@{name}{{}}"),
        Amount::Number(n) => format!("@{name}{{{n}}}"),
        Amount::Numbered(n, unit) => format!("@{name}{{{n}%{unit}}}"),
        Amount::Text => format!("@{name}{{a pinch}}"),
    }
}

/// A `.cook` document: optional frontmatter, then steps carrying zero or more
/// ingredients plus cookware and timers the row projection must ignore.
fn recipe_text() -> impl Strategy<Value = String> {
    let step = proptest::collection::vec((proptest::sample::select(NAMES), amount()), 0..4);
    (
        proptest::option::of("[A-Z][a-z]{2,10}"),
        proptest::option::of(prop_oneof![Just("breakfast"), Just("dinner")]),
        proptest::collection::vec(step, 1..5),
    )
        .prop_map(|(title, course, steps)| {
            let mut out = String::new();
            if title.is_some() || course.is_some() {
                out.push_str("---\n");
                if let Some(title) = &title {
                    out.push_str(&format!("title: {title}\n"));
                }
                if let Some(course) = course {
                    out.push_str(&format!("course: {course}\n"));
                }
                out.push_str("---\n\n");
            }
            for (index, ingredients) in steps.iter().enumerate() {
                out.push_str("Combine ");
                for (name, amount) in ingredients {
                    out.push_str(&render_ingredient(name, amount));
                    out.push(' ');
                }
                out.push_str(&format!(
                    "in a #bowl{{}} for ~{{{}%minutes}}.\n\n",
                    index + 1
                ));
            }
            out
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    /// Nothing the adapter emits is lost, added or re-typed by the contract.
    #[test]
    fn cook_rows_survive_the_json_lines_contract(text in recipe_text()) {
        let sets = rows_of("Rezepte/Generated.cook", &text);
        prop_assert_eq!(round_trip(&sets), sets);
    }

    /// Both scopes reach the wire even when one owns no rows — an
    /// `ingredient_use` scope dropped for being empty is how the LAST
    /// ingredient of a recipe would never get swept on re-ingest.
    #[test]
    fn both_scopes_reach_the_wire(text in recipe_text()) {
        let sets = rows_of("Rezepte/Generated.cook", &text);
        let back = round_trip(&sets);
        prop_assert_eq!(
            back.iter().map(|s| s.type_name.as_str()).collect::<Vec<_>>(),
            vec!["recipe", "ingredient_use"]
        );
    }
}

#[test]
fn the_pancakes_fixture_survives_the_contract() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pancakes.cook");
    let content = std::fs::read_to_string(&path).unwrap();
    let sets = rows_of("pancakes.cook", &content);
    assert_eq!(round_trip(&sets), sets);
}

/// `@maple syrup{}` has no amount, and the row must carry NULL for it — not a
/// fabricated zero, and not an absent column, either of which would let a
/// nutrition rollup read the ingredient as weightless.
#[test]
fn an_amountless_ingredient_keeps_its_null_quantity_on_the_wire() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pancakes.cook");
    let content = std::fs::read_to_string(&path).unwrap();
    let sets = rows_of("pancakes.cook", &content);
    let text = emit_row_sets(&sets).unwrap();

    let syrup = text
        .lines()
        .find(|line| line.contains("maple syrup"))
        .expect("the fixture's amountless ingredient must reach the wire");
    assert!(
        syrup.contains("\"quantity\":null"),
        "quantity must be null, not zero or absent: {syrup}"
    );

    let back = round_trip(&sets);
    let uses = back
        .iter()
        .find(|s| s.type_name == "ingredient_use")
        .unwrap();
    let row = uses
        .rows
        .iter()
        .find(|r| r.get("raw_name") == Some(&holon_api::Value::String("maple syrup".into())))
        .unwrap();
    assert_eq!(row.get("quantity"), Some(&holon_api::Value::Null));
    assert_eq!(row.get("unit"), Some(&holon_api::Value::Null));
}
