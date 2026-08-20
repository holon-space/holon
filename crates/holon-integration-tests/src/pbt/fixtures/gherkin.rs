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
use super::matchers::match_assertion;
use crate::pbt::transitions::E2ETransition;

/// One scenario (or one Outline row) flattened into an ordered step sequence.
pub struct FeatureCase {
    pub name: String,
    /// Feature-level and scenario-level tags merged, `@` already stripped by
    /// the parser (`@wip` arrives as `"wip"`).
    pub tags: Vec<String>,
    /// Empty when `skipped`.
    pub steps: Vec<FixtureStep>,
    pub skipped: bool,
}

/// A `@wip` feature or scenario is not executed. `tags` is the feature-level
/// and scenario-level tags merged.
pub fn is_wip(tags: &[String]) -> bool {
    tags.iter().any(|t| t.eq_ignore_ascii_case("wip"))
}

/// The tag tokens on the `@`-lines preceding the `Feature:` keyword, `@`
/// stripped. Read by a line scan so a `@wip` feature can be recognised and
/// skipped before its body is structurally parsed — a not-yet-implemented
/// feature need not be valid Gherkin yet.
fn feature_level_tags(text: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('@') {
            tags.extend(
                line.split_whitespace()
                    .filter_map(|t| t.strip_prefix('@'))
                    .map(str::to_string),
            );
            continue;
        }
        break;
    }
    tags
}

/// Names of every `Scenario:` / `Scenario Outline:` in source order, by line
/// scan. Used to report one skip per scenario for a `@wip` feature without
/// structurally parsing its (possibly not-yet-valid) body.
fn scenario_names(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("Scenario Outline:")
                .or_else(|| line.strip_prefix("Scenario:"))
                .map(|name| name.trim().to_string())
        })
        .collect()
}

/// Parse every scenario in `path` into one or more [`FeatureCase`]s. A plain
/// scenario yields one case; a `Scenario Outline` yields one per `Examples`
/// data row. A `@wip` feature yields one skipped case per scenario without
/// structural parsing. Parse errors and unmatched steps are returned as `Err`
/// (caller fails loud).
pub fn parse_feature_file(path: impl AsRef<Path>) -> Result<Vec<FeatureCase>, String> {
    let path = path.as_ref();
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let feature_tags = feature_level_tags(&text);
    if is_wip(&feature_tags) {
        return Ok(scenario_names(&text)
            .into_iter()
            .map(|name| FeatureCase {
                name,
                tags: feature_tags.clone(),
                steps: Vec::new(),
                skipped: true,
            })
            .collect());
    }

    let feature = Feature::parse_path(path, GherkinEnv::default())
        .map_err(|e| format!("parse {}: {e}", path.display()))?;

    let mut cases = Vec::new();
    for scenario in &feature.scenarios {
        let mut tags = feature.tags.clone();
        tags.extend(scenario.tags.iter().cloned());

        if is_wip(&tags) {
            // Steps stay unparsed: a `@wip` scenario may use phrasings the
            // step registry does not know yet, and `build_case` fails loud on
            // an unknown step. The runner reports it skipped.
            cases.push(FeatureCase {
                name: scenario.name.clone(),
                tags,
                steps: Vec::new(),
                skipped: true,
            });
            continue;
        }

        // Background first, then the scenario's own steps. Re-materialised
        // per Outline row so each row sees its own substitutions.
        let steps: Vec<&Step> = feature
            .background
            .iter()
            .flat_map(|b| b.steps.iter())
            .chain(scenario.steps.iter())
            .collect();

        if scenario.examples.is_empty() {
            cases.push(build_case(
                path,
                &scenario.name,
                &steps,
                &HashMap::new(),
                &tags,
            )?);
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
                cases.push(build_case(path, &name, &steps, &subs, &tags)?);
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
    tags: &[String],
) -> Result<FeatureCase, String> {
    let mut out = Vec::new();
    for step in steps {
        let substituted = substitute_step(step, subs);
        let fixture_step = match substituted.ty {
            // `And`/`But` are already resolved to the prior concrete type.
            StepType::Given | StepType::When => FixtureStep::Action(
                E2ETransition::parse_step(
                    substituted.value.trim(),
                    substituted.docstring.as_deref(),
                )
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
        tags: tags.to_vec(),
        steps: out,
        skipped: false,
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
                    skipped: case.skipped,
                });
            }
        }
        out
    }
}
