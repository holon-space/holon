//! A recipe's front-matter value is kept or refused by name, never degraded.
//!
//! Every value the plugin keeps converts back to the YAML value the file held
//! (a `.cook` writer's contract), and the plugin refuses a recipe exactly when
//! some value has no lossless single-text form.

use std::path::PathBuf;
use std::sync::LazyLock;

use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_plugin_host::PluginFormatAdapter;
use proptest::prelude::*;
use serde_yaml::Mapping;
use serde_yaml::Value as Yaml;

use crate::support;

// Only the reverse conversion is called here; the forward half runs inside the
// wasm guest.
#[allow(dead_code)]
#[path = "../../../guests/cooklang/src/metadata.rs"]
mod metadata;

static PLUGIN: LazyLock<PluginFormatAdapter> = LazyLock::new(support::bundled_cook_plugin);

/// A container value JSON holds losslessly: string map keys, no tags, finite
/// numbers. A scalar or scalar list never reaches it; those keep their YAML
/// text.
fn representable(value: &Yaml) -> bool {
    match value {
        Yaml::Tagged(_) => false,
        Yaml::Number(n) => n.as_f64().is_none_or(f64::is_finite),
        Yaml::Sequence(items) => items.iter().all(representable),
        Yaml::Mapping(map) => map
            .iter()
            .all(|(key, item)| key.is_string() && representable(item)),
        _ => true,
    }
}

fn is_container(value: &Yaml) -> bool {
    matches!(value, Yaml::Sequence(_) | Yaml::Mapping(_))
}

/// The text a property holds, or `None` when the value is refused.
fn expected_text(value: &Yaml) -> Option<String> {
    let scalar = |v: &Yaml| match v {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Number(n) => Some(n.to_string()),
        Yaml::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    match value {
        Yaml::Sequence(items) if !items.iter().any(is_container) => items
            .iter()
            .map(scalar)
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.join(", ")),
        Yaml::Sequence(_) | Yaml::Mapping(_) => representable(value).then(String::new),
        other => scalar(other),
    }
}

fn check(front_matter: &str) -> Result<(), TestCaseError> {
    let source: Mapping = serde_yaml::from_str(front_matter).unwrap();
    let content = format!("---\n{front_matter}---\n\nBoil @water{{1%l}}.\n");
    let root = PathBuf::from("/vault");
    let parsed = PLUGIN.parse(
        &root.join("Rezepte/T.cook"),
        &content,
        &EntityUri::no_parent(),
        &root,
    );

    let refused = source
        .iter()
        .any(|(key, value)| !key.is_string() || expected_text(value).is_none());
    let parsed = match (parsed, refused) {
        (Ok(parsed), false) => parsed,
        (Err(e), true) => {
            let msg = format!("{e:#}");
            let first = source
                .iter()
                .find(|(key, value)| !key.is_string() || expected_text(value).is_none())
                .unwrap();
            if let Some(name) = first.0.as_str() {
                prop_assert!(msg.contains(name), "refusal omits {name:?}: {msg}");
            }
            return Ok(());
        }
        (Ok(_), true) => return Err(TestCaseError::fail("a lossy value was imported")),
        (Err(e), false) => return Err(TestCaseError::fail(format!("refused: {e:#}"))),
    };

    for (key, value) in &source {
        let name = key.as_str().unwrap();
        let text = parsed.document.get_property_str(name);
        let text = text.unwrap_or_else(|| panic!("property {name:?} missing"));
        match value {
            Yaml::Sequence(items) if items.iter().any(is_container) => {
                let back = metadata::yaml_from_property_text(&text).unwrap();
                prop_assert_eq!(
                    serde_yaml::to_string(&back).unwrap(),
                    serde_yaml::to_string(value).unwrap()
                );
                prop_assert_eq!(&back, value);
            }
            Yaml::Mapping(_) => {
                let back = metadata::yaml_from_property_text(&text).unwrap();
                prop_assert_eq!(
                    serde_yaml::to_string(&back).unwrap(),
                    serde_yaml::to_string(value).unwrap()
                );
                prop_assert_eq!(&back, value);
            }
            _ => prop_assert_eq!(Some(text), expected_text(value)),
        }
    }
    Ok(())
}

/// Cases a generator found or the contract names; each runs through the same
/// property before the random ones.
const STATIC_CASES: &[&str] = &[
    "nutrition:\n  kcal: 200\n",
    "nutrition:\n  1: x\n",
    "nutrition:\n  ? [a]\n  : x\n",
    "kind: !foo bar\n",
    "outer:\n  inner: !foo bar\n",
    "1: x\n",
    "tm_fill_ml: {bowl: 1855}\n",
    "steps:\n  - {id: a1, n: 1.0}\n  - [x, y]\n",
    "outer: {a: 1,\n    b: [x, {c: 2}]}\n",
    "brace: \"{\"\n",
    "tags: [quick, vegan]\n",
    "empty:\n",
    "plain: [a, null]\n",
    "outer:\n  v: .nan\n",
    "outer: [1, [.inf]]\n",
];

#[test]
fn the_static_cases_hold() {
    for case in STATIC_CASES {
        check(case).unwrap_or_else(|e| panic!("{case:?}: {e}"));
    }
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(128) })]

    #[test]
    fn a_front_matter_value_is_kept_losslessly_or_refused_by_name(front in support::front_matter()) {
        check(&serde_yaml::to_string(&front).unwrap())?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(64) })]

    #[test]
    fn a_body_brace_left_open_is_refused_behind_any_front_matter(
        front in support::front_matter(),
        blank_line in any::<bool>(),
    ) {
        let yaml = serde_yaml::to_string(&front).unwrap();
        let gap = if blank_line { "\n" } else { "" };
        let content = format!("---\n{yaml}---\n{gap}Add the @flour{{200%g to the bowl.\n");
        let root = PathBuf::from("/vault");
        let msg = PLUGIN
            .parse(&root.join("Rezepte/T.cook"), &content, &EntityUri::no_parent(), &root)
            .err()
            .map(|e| format!("{e:#}"));
        prop_assert!(
            msg.as_deref().is_some_and(|m| m.contains("brace")),
            "an open body brace behind front matter must be refused by the brace guard: {msg:?}"
        );
    }
}
