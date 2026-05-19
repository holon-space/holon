//! JSON fixtures: machine-captured shrunk counterexamples, one fixture per
//! `*.json` file under a directory.

use std::path::{Path, PathBuf};

use holon_pbt_core::fixture::Fixture;

use super::{FixtureSource, FixtureStep, NamedFixture};
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
        let fixture: Fixture<E2ETransition, ()> =
            Fixture::load(&path).unwrap_or_else(|e| panic!("[json] load {path:?}: {e}"));
        out.push(NamedFixture {
            name: fixture.name,
            description: fixture.description,
            steps: fixture
                .transitions
                .into_iter()
                .map(FixtureStep::Action)
                .collect(),
        });
    }
    out
}
