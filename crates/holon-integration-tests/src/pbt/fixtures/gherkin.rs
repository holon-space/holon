//! Parse a Gherkin `.feature` file into replayable transition sequences.
//!
//! Actions only (`Given`/`When`); `Then` assertions arrive with the assert
//! vocabulary. Supported: `Background` (prepended to every scenario, re-run
//! per Outline row) and `Scenario Outline` + `Examples` (expanded to one
//! fixture per data row, with `<placeholder>` substituted in step text,
//! docstrings, and table cells).

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use gherkin::Feature;
use gherkin::GherkinEnv;
use gherkin::Step;
use gherkin::StepType;

use super::FixtureSource;
use super::FixtureStep;
use super::NamedFixture;
use super::matchers::match_action;
use super::matchers::match_assertion;

/// One scenario (or one Outline row) flattened into an ordered step sequence.
pub struct FeatureCase {
    pub name: String,
    pub steps: Vec<FixtureStep>,
}

/// Parse every scenario in `path` into one or more [`FeatureCase`]s. A plain
/// scenario yields one case; a `Scenario Outline` yields one per `Examples`
/// data row. Parse errors and unmatched steps are returned as `Err` (caller
/// fails loud).
pub fn parse_feature_file(path: impl AsRef<Path>) -> Result<Vec<FeatureCase>, String> {
    let path = path.as_ref();
    let feature = Feature::parse_path(path, GherkinEnv::default())
        .map_err(|e| format!("parse {}: {e}", path.display()))?;

    let mut cases = Vec::new();
    for scenario in &feature.scenarios {
        // Background first, then the scenario's own steps. Re-materialised
        // per Outline row so each row sees its own substitutions.
        let steps: Vec<&Step> = feature
            .background
            .iter()
            .flat_map(|b| b.steps.iter())
            .chain(scenario.steps.iter())
            .collect();

        if scenario.examples.is_empty() {
            cases.push(build_case(path, &scenario.name, &steps, &HashMap::new())?);
            continue;
        }

        for examples in &scenario.examples {
            let Some(table) = &examples.table else {
                continue;
            };
            let Some((headers, rows)) = table.rows.split_first() else {
                continue;
            };
            for (ri, row) in rows.iter().enumerate() {
                let subs: HashMap<&str, &str> = headers
                    .iter()
                    .zip(row.iter())
                    .map(|(h, v)| (h.as_str(), v.as_str()))
                    .collect();
                let name = format!("{} [example {}: {}]", scenario.name, ri + 1, row.join(", "));
                cases.push(build_case(path, &name, &steps, &subs)?);
            }
        }
    }
    Ok(cases)
}

fn build_case(
    path: &Path,
    name: &str,
    steps: &[&Step],
    subs: &HashMap<&str, &str>,
) -> Result<FeatureCase, String> {
    let mut out = Vec::new();
    for step in steps {
        let substituted = substitute_step(step, subs);
        let fixture_step = match substituted.ty {
            // `And`/`But` are already resolved to the prior concrete type.
            StepType::Given | StepType::When => FixtureStep::Action(
                match_action(&substituted)
                    .map_err(|e| format!("{}: scenario {name:?}: {e}", path.display()))?,
            ),
            StepType::Then => FixtureStep::Assert(
                match_assertion(&substituted)
                    .map_err(|e| format!("{}: scenario {name:?}: {e}", path.display()))?,
            ),
        };
        out.push(fixture_step);
    }
    Ok(FeatureCase {
        name: name.to_string(),
        steps: out,
    })
}

/// Clone a step with `<placeholder>` substituted in its value, docstring, and
/// table cells. An empty `subs` map is an identity transform.
fn substitute_step(step: &Step, subs: &HashMap<&str, &str>) -> Step {
    let mut step = step.clone();
    step.value = substitute(&step.value, subs);
    step.docstring = step.docstring.map(|d| substitute(&d, subs));
    if let Some(table) = step.table.as_mut() {
        for row in &mut table.rows {
            for cell in row {
                *cell = substitute(cell, subs);
            }
        }
    }
    step
}

fn substitute(text: &str, subs: &HashMap<&str, &str>) -> String {
    let mut out = text.to_string();
    for (key, value) in subs {
        out = out.replace(&format!("<{key}>"), value);
    }
    out
}

/// A [`FixtureSource`] over `*.feature` files. The path may be a single file
/// or a directory (every `*.feature` within is loaded). A missing path yields
/// zero fixtures (no-op).
pub struct GherkinFixtureSource {
    path: PathBuf,
}

impl GherkinFixtureSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn feature_files(&self) -> Vec<PathBuf> {
        if self.path.is_file() {
            return vec![self.path.clone()];
        }
        if self.path.is_dir() {
            let entries = std::fs::read_dir(&self.path)
                .unwrap_or_else(|e| panic!("[gherkin] read {:?}: {e}", self.path));
            return entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("feature"))
                .collect();
        }
        eprintln!(
            "[gherkin] {:?} does not exist — no features to replay",
            self.path
        );
        Vec::new()
    }
}

impl FixtureSource for GherkinFixtureSource {
    fn kind(&self) -> &'static str {
        "gherkin"
    }

    fn load(&self) -> Vec<NamedFixture> {
        let mut out = Vec::new();
        for file in self.feature_files() {
            let cases = parse_feature_file(&file).unwrap_or_else(|e| panic!("[gherkin] {e}"));
            for case in cases {
                out.push(NamedFixture {
                    name: case.name,
                    description: file.display().to_string(),
                    wiring: None,
                    env_flags: Default::default(),
                    steps: case.steps,
                });
            }
        }
        out
    }
}
