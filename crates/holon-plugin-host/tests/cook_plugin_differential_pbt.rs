//! Differential: the cooklang PLUGIN projects a `.cook` file exactly as the
//! native `CookFormatAdapter` does — rows, document and blocks alike.
//!
//! This is what makes Inc 3's deletion of `cook.rs` safe: anything the plugin
//! projects differently is a projection the vault would silently change under
//! the user.
//!
//! THE DIVERGENCE RULE. "New equals old" alone would silently bless an
//! upstream-versus-ours parse difference as correct. So a divergence is never
//! allowlisted here: it fails unless [`FILED_DIVERGENCES`] names a bugfunnel
//! entry that EXISTS on disk and carries a ruling on which side is wrong. An
//! empty table therefore means the differential found nothing, not that
//! nothing was looked for.

use std::path::PathBuf;

use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_kitchen::CookFormatAdapter;
use proptest::prelude::*;

mod support;

/// Divergences carrying a ruling, each naming its bugfunnel entry. The entry
/// file's existence is asserted, so nothing can be excused by editing this
/// list alone.
const FILED_DIVERGENCES: &[Divergence] = &[];

struct Divergence {
    /// The bugfunnel entry id under `docs/Testing/bugfunnel/entries/`.
    #[allow(dead_code)]
    entry: &'static str,
    /// Recipe sources this ruling covers.
    #[allow(dead_code)]
    covers: fn(&str) -> bool,
}

fn bugfunnel_entries_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/Testing/bugfunnel/entries")
}

/// Both legs' verdict on one recipe, reduced to what a differential can
/// compare: an `Err` is its own outcome, because refusing a file the other leg
/// accepts is the single largest divergence there is.
fn project(
    adapter: &dyn FileFormatAdapter,
    rel: &str,
    content: &str,
) -> Result<FileFormatParseResult, String> {
    let root = PathBuf::from("/vault");
    adapter
        .parse(&root.join(rel), content, &EntityUri::no_parent(), &root)
        .map_err(|e| format!("{e:#}"))
}

/// Compare the two projections, returning what differs — nothing, or a named
/// difference.
fn compare(
    native: &Result<FileFormatParseResult, String>,
    plugin: &Result<FileFormatParseResult, String>,
) -> Option<String> {
    match (native, plugin) {
        (Err(_), Err(_)) => None,
        (Ok(_), Err(e)) => Some(format!(
            "the plugin refuses a file the native adapter accepts: {e}"
        )),
        (Err(e), Ok(_)) => Some(format!(
            "the plugin accepts a file the native adapter refuses with: {e}"
        )),
        (Ok(native), Ok(plugin)) => {
            if native.typed_rows != plugin.typed_rows {
                return Some(format!(
                    "rows differ:\n  native: {:#?}\n  plugin: {:#?}",
                    native.typed_rows, plugin.typed_rows
                ));
            }
            if native.document.content != plugin.document.content
                || native.document.properties != plugin.document.properties
                || native.document.id != plugin.document.id
            {
                return Some(format!(
                    "document differs:\n  native: {:?} {:?}\n  plugin: {:?} {:?}",
                    native.document.id,
                    native.document.properties,
                    plugin.document.id,
                    plugin.document.properties
                ));
            }
            let shape = |r: &FileFormatParseResult| {
                r.blocks
                    .iter()
                    .map(|b| (b.id.to_string(), b.content.clone(), b.properties.clone()))
                    .collect::<Vec<_>>()
            };
            if shape(native) != shape(plugin) {
                return Some(format!(
                    "blocks differ:\n  native: {:#?}\n  plugin: {:#?}",
                    shape(native),
                    shape(plugin)
                ));
            }
            None
        }
    }
}

/// Fail unless a filed, on-disk bugfunnel entry rules on this divergence.
fn assert_ruled(content: &str, difference: &str) {
    let filed = FILED_DIVERGENCES.iter().find(|d| (d.covers)(content));
    let Some(filed) = filed else {
        panic!(
            "UNFILED DIVERGENCE — triage it with the bug-gap-triage skill and add its entry to \
             FILED_DIVERGENCES before this passes.\nrecipe:\n{content}\n{difference}"
        );
    };
    let entry = bugfunnel_entries_dir().join(format!("{}.md", filed.entry));
    assert!(
        entry.is_file(),
        "divergence excused by entry {} — which does not exist at {}",
        filed.entry,
        entry.display()
    );
}

fn assert_same(rel: &str, content: &str) {
    let native = project(&CookFormatAdapter::new(), rel, content);
    let plugin = project(&support::cook_plugin(), rel, content);
    if let Some(difference) = compare(&native, &plugin) {
        assert_ruled(content, &difference);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(96),
        ..ProptestConfig::default()
    })]

    /// The whole projection — rows, document, blocks — is the same on both
    /// legs, or the difference is ruled on.
    #[test]
    fn the_plugin_projects_a_recipe_exactly_as_the_native_adapter(text in support::recipe_text()) {
        let native = project(&CookFormatAdapter::new(), "Rezepte/Generated.cook", &text);
        let plugin = project(&support::cook_plugin(), "Rezepte/Generated.cook", &text);
        // Two refusals agree trivially. The generator only produces recipes
        // the native adapter accepts, so a native `Err` here means the
        // comparison stopped comparing anything.
        prop_assert!(
            native.is_ok(),
            "the generator produced a recipe the native adapter refuses, so this case proves nothing: {:?}",
            native.as_ref().err()
        );
        if let Some(difference) = compare(&native, &plugin) {
            assert_ruled(&text, &difference);
        }
    }
}

#[test]
fn the_pancakes_fixture_projects_identically() {
    let content = support::pancakes_fixture();
    let native = project(&CookFormatAdapter::new(), "pancakes.cook", &content)
        .expect("the fixture is a valid recipe, so a refusal here compares nothing");
    assert!(!native.typed_rows.is_empty() && !native.blocks.is_empty());
    assert_same("pancakes.cook", &content);
}

/// The one plugin instance is reused across files, which is the whole point of
/// keeping the host alive — and a guest that leaked state between calls would
/// show up here as a second file projecting differently from the first.
#[test]
fn every_generated_recipe_projects_identically_through_one_host() {
    let plugin = support::cook_plugin();
    let native = CookFormatAdapter::new();
    for (rel, content) in support::generated_vault(40) {
        let expected = project(&native, &rel, &content);
        let actual = project(&plugin, &rel, &content);
        assert!(
            expected.is_ok(),
            "the vault generator must produce recipes the native adapter accepts: {:?}",
            expected.as_ref().err()
        );
        if let Some(difference) = compare(&expected, &actual) {
            assert_ruled(&content, &difference);
        }
    }
}

/// A German timer unit (`~{9%Minuten}`) — bugfunnel entry
/// `2026-09-02-a-german-timer-unit-refuses-the-whole-recipe`. The plan expected
/// the upstream crate to ACCEPT it, making this the first divergence; it does
/// not, because `cooklang::parse` enables `Extensions::ADVANCED_UNITS` and
/// validates timer units against a built-in English table. Pinned so the day a
/// ruling changes one leg, the differential says so.
#[test]
fn a_german_timer_unit_is_refused_by_both_legs() {
    let recipe = "Koche @Mehl{200%g} fuer ~{9%Minuten}.\n";
    let native = project(&CookFormatAdapter::new(), "Rezepte/Deutsch.cook", recipe);
    let plugin = project(&support::cook_plugin(), "Rezepte/Deutsch.cook", recipe);

    let Err(native) = native else {
        panic!("cooklang refuses an unknown timer unit, so the native adapter must too")
    };
    let Err(plugin) = plugin else {
        panic!("the plugin must refuse an unknown timer unit for the same reason")
    };
    assert!(
        native.contains("Unknown timer unit") && plugin.contains("Unknown timer unit"),
        "both legs must refuse for the SAME reason:\n  native: {native}\n  plugin: {plugin}"
    );
    assert!(
        compare(&Err(native), &Err(plugin)).is_none(),
        "two refusals are not a divergence"
    );
}
