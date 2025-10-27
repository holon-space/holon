//! JSON fixtures: machine-captured shrunk counterexamples, one fixture per
//! `*.json` file under a directory.

use std::path::Path;
use std::path::PathBuf;

use holon_pbt_core::fixture::Fixture;

use super::FixtureSource;
use super::FixtureStep;
use super::NamedFixture;
use crate::pbt::transitions::E2ETransition;

pub struct JsonFixtureSource {
    dir: PathBuf,
}

impl JsonFixtureSource {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

impl FixtureSource for JsonFixtureSource {
    fn kind(&self) -> &'static str {
        "json"
    }

    fn load(&self) -> Vec<NamedFixture> {
        load_dir(&self.dir)
    }
}

/// Load a single `Fixture<E2ETransition>` JSON file (the capture-on-panic
/// format) into a [`NamedFixture`] of `Action` steps. Shared by the directory
/// source and by single-file consumers (e.g. the windowed capture-replay test).
pub fn load_file(path: &Path) -> NamedFixture {
    let fixture: Fixture<E2ETransition, ()> =
        Fixture::load(path).unwrap_or_else(|e| panic!("[json] load {path:?}: {e}"));
    NamedFixture {
        name: fixture.name,
        description: fixture.description,
        wiring: fixture.environment.wiring,
        env_flags: fixture.environment.env_flags,
        steps: fixture
            .transitions
            .into_iter()
            .map(FixtureStep::Action)
            .collect(),
    }
}

fn load_dir(dir: &Path) -> Vec<NamedFixture> {
    if !dir.exists() {
        eprintln!("[json] {dir:?} does not exist — no fixtures to replay");
        return Vec::new();
    }
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("[json] read {dir:?}: {e}"));
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        out.push(load_file(&path));
    }
    out
}
