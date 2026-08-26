//! Every shipped `.feature` file reads through the ONE action vocabulary.
//!
//! The 6 headless files plus the windowed GPUI one must each resolve every
//! `Given`/`When` step to exactly one transition through the generated step
//! registry. Until the regexes in `pbt::fixtures::matchers` were deleted this
//! file also asserted both parsers AGREED on every step, value for value; that
//! transitional test is what proved the migration lossless (lane log
//! `lane-vocab-4.log`).

#![cfg(feature = "pbt")]

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use gherkin::Feature;
use gherkin::GherkinEnv;
use gherkin::Step;
use gherkin::StepType;
use holon_integration_tests::pbt::transitions::E2ETransition;

/// Every `.feature` file the tree ships, headless and windowed.
fn feature_files() -> Vec<PathBuf> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_root.parent().unwrap().parent().unwrap();
    let files: Vec<PathBuf> = vec![
        crate_root.join("tests/fixtures/split_block_content_pbt/split_outline_positions.feature"),
        crate_root.join("tests/fixtures/_gherkin_assert/widget_and_focus.feature"),
        crate_root.join("tests/fixtures/_gherkin_assert/split_then_address_new_block.feature"),
        crate_root.join("tests/fixtures/_gherkin_assert/parentage.feature"),
        crate_root.join("tests/fixtures/composed_split_gherkin/split_routes_prefix_suffix.feature"),
        crate_root.join("tests/fixtures/_gherkin_negative/then_before_startup.feature"),
        crate_root.join("tests/fixtures/_gherkin_negative/split_corrupt_id.feature"),
        crate_root.join("tests/fixtures/_gherkin_negative/switch_view_mode_no_surface.feature"),
        repo_root.join("frontends/gpui/tests/features/ordinary_block_interaction.feature"),
    ];
    for f in &files {
        assert!(f.is_file(), "feature file missing: {}", f.display());
    }
    files
}

/// Flatten every scenario (Background + Outline rows expanded) into the
/// action steps a replay would see — the same flattening
/// `pbt::fixtures::gherkin::parse_feature_file` performs.
fn action_steps(path: &Path) -> Vec<Step> {
    let feature = Feature::parse_path(path, GherkinEnv::default())
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let mut out = Vec::new();
    for scenario in &feature.scenarios {
        let steps: Vec<&Step> = feature
            .background
            .iter()
            .flat_map(|b| b.steps.iter())
            .chain(scenario.steps.iter())
            .collect();
        let mut subs_list: Vec<HashMap<String, String>> = Vec::new();
        for examples in &scenario.examples {
            let Some(table) = &examples.table else {
                continue;
            };
            let Some((headers, rows)) = table.rows.split_first() else {
                continue;
            };
            for row in rows {
                subs_list.push(
                    headers
                        .iter()
                        .cloned()
                        .zip(row.iter().cloned())
                        .collect::<HashMap<_, _>>(),
                );
            }
        }
        if subs_list.is_empty() {
            subs_list.push(HashMap::new());
        }
        for subs in &subs_list {
            for step in &steps {
                if !matches!(step.ty, StepType::Given | StepType::When) {
                    continue;
                }
                let mut step = (*step).clone();
                step.value = substitute(&step.value, subs);
                step.docstring = step.docstring.map(|d| substitute(&d, subs));
                out.push(step);
            }
        }
    }
    out
}

fn substitute(text: &str, subs: &HashMap<String, String>) -> String {
    let mut out = text.to_string();
    for (k, v) in subs {
        out = out.replace(&format!("<{k}>"), v);
    }
    out
}

/// The registry alone resolves every shipped action step to exactly one
/// transition.
#[test]
fn registry_parses_every_feature_file_step() {
    let mut parsed = 0usize;
    for file in feature_files() {
        for step in action_steps(&file) {
            E2ETransition::parse_step(step.value.trim(), step.docstring.as_deref())
                .unwrap_or_else(|e| panic!("{}: registry parse: {e}", file.display()));
            parsed += 1;
        }
    }
    assert!(
        parsed >= 15,
        "expected at least 15 action steps across the 7 shipped feature files, saw {parsed}"
    );
}
