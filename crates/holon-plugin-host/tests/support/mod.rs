//! Shared fixtures for the plugin-host suites: the installed cooklang plugin,
//! and the recipe generator both differential legs are driven with.

// Each test binary compiles the whole module and uses a different part of it.
#![allow(dead_code)]

use std::path::Path;
use std::path::PathBuf;

use holon_plugin_host::PluginFormatAdapter;
use holon_plugin_host::PluginLimits;
use proptest::prelude::*;

pub fn plugins_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins")
}

/// The installed cooklang plugin, loaded from its sidecar exactly as the
/// registry would.
pub fn cook_plugin() -> PluginFormatAdapter {
    PluginFormatAdapter::load(
        &plugins_dir().join("cooklang.yaml"),
        PluginLimits::default(),
    )
    .expect("the cooklang plugin must load from its sidecar")
}

pub fn pancakes_fixture() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../holon-kitchen/tests/fixtures/pancakes.cook");
    std::fs::read_to_string(&path).expect("the pancakes fixture must be readable")
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
pub enum Amount {
    /// `@salt` — no braces at all.
    Bare,
    /// `@salt{}` — an empty amount, never a zero.
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
pub fn recipe_text() -> impl Strategy<Value = String> {
    let step = proptest::collection::vec((proptest::sample::select(NAMES), amount()), 0..4);
    (
        proptest::option::of("[A-Z][a-z]{2,10}"),
        proptest::option::of(prop_oneof![Just("breakfast"), Just("dinner")]),
        proptest::collection::vec(step, 1..5),
    )
        .prop_map(|(title, course, steps)| render_recipe(title.as_deref(), course, &steps))
}

fn render_recipe(
    title: Option<&str>,
    course: Option<&str>,
    steps: &[Vec<(&str, Amount)>],
) -> String {
    let mut out = String::new();
    if title.is_some() || course.is_some() {
        out.push_str("---\n");
        if let Some(title) = title {
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
}

/// `count` recipes of a fixed shape — the vault-scan workload, generated
/// without proptest so the timing run is deterministic.
pub fn generated_vault(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|i| {
            let steps: Vec<Vec<(&str, Amount)>> = (0..4)
                .map(|s| {
                    (0..3)
                        .map(|j| {
                            (
                                NAMES[(i + s + j) % NAMES.len()],
                                Amount::Numbered(100 + (j as u32 * 7), "g"),
                            )
                        })
                        .collect()
                })
                .collect();
            (
                format!("Rezepte/Generated{i}.cook"),
                render_recipe(Some("Generated"), Some("dinner"), &steps),
            )
        })
        .collect()
}

/// One recipe of `steps` steps — the large input the fuel and memory budgets
/// are sized against.
pub fn big_recipe(steps: usize) -> String {
    let shape: Vec<Vec<(&str, Amount)>> = (0..steps)
        .map(|s| {
            (0..3)
                .map(|j| {
                    (
                        NAMES[(s + j) % NAMES.len()],
                        Amount::Numbered(100 + j as u32, "g"),
                    )
                })
                .collect()
        })
        .collect();
    render_recipe(Some("Large"), Some("dinner"), &shape)
}
