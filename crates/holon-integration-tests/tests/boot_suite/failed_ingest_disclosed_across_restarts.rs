//! A file whose ingest failed is disclosed on every boot until an ingest of
//! it succeeds, whatever the user does to it while Holon is off.
//!
//! The failure is forced by a seam that fails every ingest of one file after
//! its blocks are written, as a real partial ingest would.
//!
//! @pbt kind stateful
//! @pbt covers failed-ingest-disclosure — a failed ingest stays disclosed
//! across restarts and edits made while Holon is off, and clears on the next
//! ingest that succeeds
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone has no seam
//! that fails an ingest after its blocks are written

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use holon_filesystem::FileSystem;
use holon_filesystem::crash_injection;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);
const FILES: [&str; 2] = ["Alpha.org", "Beta.org"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Bytes {
    Variant(u8),
    /// The bytes the file held after its last ingest that succeeded.
    Revert,
}

#[derive(Debug, Clone)]
enum Step {
    Edit(usize, Bytes),
    FailIngests(usize, bool),
    /// Stop, write these while Holon is off, boot again.
    Restart(Vec<(usize, Bytes)>),
}

/// Variant 2 adds a block, so an edit is a structural change half the time.
fn variant(file: usize, v: u8) -> String {
    let key = FILES[file].trim_end_matches(".org").to_lowercase();
    let extra = if v == 2 {
        format!("** Extra\n:PROPERTIES:\n:ID: {key}-extra\n:END:\n")
    } else {
        String::new()
    };
    format!(
        "* Root {v}\n:PROPERTIES:\n:ID: {key}-root\n:END:\n** Child {v}\n:PROPERTIES:\n:ID: \
         {key}-child\n:END:\n{extra}"
    )
}

fn bytes() -> impl Strategy<Value = Bytes> {
    prop_oneof![(0u8..3).prop_map(Bytes::Variant), Just(Bytes::Revert)]
}

fn step() -> impl Strategy<Value = Step> {
    let file = 0..FILES.len();
    prop_oneof![
        (file.clone(), bytes()).prop_map(|(f, b)| Step::Edit(f, b)),
        (file.clone(), any::<bool>()).prop_map(|(f, on)| Step::FailIngests(f, on)),
        prop::collection::vec((file, bytes()), 0..3).prop_map(Step::Restart),
    ]
}

#[derive(Debug, Default, Clone)]
struct FileModel {
    /// What the disk held once the last successful ingest settled.
    clean: Option<String>,
    refused: bool,
    failing: bool,
}

struct Run {
    env: TestEnvironment,
    paths: Vec<PathBuf>,
    model: Vec<FileModel>,
}

impl Run {
    async fn disk(&self, file: usize) -> String {
        self.env
            .org_fs
            .read_to_string(&self.paths[file])
            .await
            .unwrap_or_else(|e| panic!("read {}: {e}", FILES[file]))
    }

    fn content(&self, file: usize, bytes: Bytes) -> Option<String> {
        match bytes {
            Bytes::Variant(v) => Some(variant(file, v)),
            Bytes::Revert => self.model[file].clean.clone(),
        }
    }

    fn disclosed(&self) -> BTreeMap<&'static str, bool> {
        let refused: Vec<String> = self
            .env
            .injector()
            .expect("a booted app has an injector")
            .resolve::<Arc<holon_conditions::ConditionBus>>()
            .refused_files("org")
            .iter()
            .map(|f| f.path.clone())
            .collect();
        FILES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let path = self.paths[i].to_string_lossy();
                (*name, refused.iter().any(|r| *r == path))
            })
            .collect()
    }

    /// The model's refusal follows an attempt that the file's state forces;
    /// where the boot may skip a clean file, the observation decides.
    async fn after_attempt(&mut self, file: usize, forced: bool, observed: bool) {
        let failing = self.model[file].failing;
        if forced || observed == failing {
            self.model[file].refused = failing;
            if !failing {
                self.model[file].clean = Some(self.disk(file).await);
            }
        }
    }

    async fn apply(&mut self, step: &Step) {
        match step {
            Step::Edit(file, bytes) => {
                let Some(content) = self.content(*file, *bytes) else {
                    return;
                };
                // Bytes equal to the disk's change nothing, so no ingest is owed.
                let changed = self.disk(*file).await != content;
                self.env
                    .write_org_file(FILES[*file], &content)
                    .await
                    .expect("write a vault file");
                let seq = self.env.org_fs.last_change_seq();
                self.env
                    .wait_for_org_change_processed(seq, SYNC_TIMEOUT)
                    .await;
                self.env.settle().await;
                let observed = self.disclosed()[FILES[*file]];
                self.after_attempt(*file, changed, observed).await;
            }
            Step::FailIngests(file, on) => {
                self.model[*file].failing = *on;
                if *on {
                    crash_injection::fail_ingests_of(self.paths[*file].clone());
                } else {
                    crash_injection::stop_failing_ingests(&self.paths[*file]);
                }
            }
            Step::Restart(offline) => {
                // A refused file is held back from write-back, so a stop that
                // still has its store edits to write reports that file; the
                // session is down either way.
                let started = std::time::Instant::now();
                let stopped = self.env.stop_app().await;
                // Every write of this test lands, so only a failed ingest can
                // be owed, and a failed ingest holds no stop.
                assert!(
                    started.elapsed() < holon_app::session::WRITEBACK_SETTLE_BUDGET,
                    "the write-back never settled: a stop with only a failed ingest owed \
                     waited out the budget, {:?} ({stopped:?})",
                    started.elapsed()
                );
                if let Err(e) = stopped {
                    let message = format!("{e:#}");
                    assert!(
                        (0..FILES.len()).any(|f| self.model[f].refused
                            && message.contains(&*self.paths[f].to_string_lossy())),
                        "a stop may fail only over a refused file's write-back: {message}"
                    );
                }
                for (file, bytes) in offline {
                    if let Some(content) = self.content(*file, *bytes) {
                        self.env
                            .write_org_file(FILES[*file], &content)
                            .await
                            .expect("write a vault file while the app is off");
                    }
                }
                let mut forced = Vec::new();
                for file in 0..FILES.len() {
                    let disk = self.disk(file).await;
                    forced.push(
                        self.model[file].refused || self.model[file].clean.as_ref() != Some(&disk),
                    );
                }
                self.env.start_app(true).await.expect("boot the app again");
                self.env.settle().await;
                let disclosed = self.disclosed();
                for (file, forced) in forced.into_iter().enumerate() {
                    self.after_attempt(file, forced, disclosed[FILES[file]])
                        .await;
                }
            }
        }
    }

    fn check(&self, step: &Step) -> Result<(), TestCaseError> {
        let expected: BTreeMap<&'static str, bool> = FILES
            .iter()
            .zip(&self.model)
            .map(|(name, m)| (*name, m.refused))
            .collect();
        prop_assert_eq!(
            self.disclosed(),
            expected,
            "after {:?}: a file is disclosed exactly while its last ingest attempt failed",
            step
        );
        Ok(())
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_test_writer()
        .try_init();
}

fn check(steps: &[Step]) -> Result<(), TestCaseError> {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build a runtime"),
    );
    rt.clone().block_on(async {
        let mut builder = TestEnvironmentBuilder::new();
        for (file, name) in FILES.iter().enumerate() {
            builder = builder.with_org_file(*name, variant(file, 0));
        }
        let env = builder.build(rt.clone()).await.expect("boot the vault");
        env.settle().await;
        let paths: Vec<PathBuf> = FILES.iter().map(|f| env.org_file_path(f)).collect();
        let mut run = Run {
            env,
            paths,
            model: vec![FileModel::default(); FILES.len()],
        };
        for file in 0..FILES.len() {
            run.after_attempt(file, true, false).await;
        }
        let mut verdict = run.check(&Step::Restart(Vec::new()));
        for step in steps {
            if verdict.is_err() {
                break;
            }
            run.apply(step).await;
            verdict = run.check(step);
        }
        for path in &run.paths {
            crash_injection::stop_failing_ingests(path);
        }
        verdict
    })
}

/// The ingest fails partway, then the user puts back the bytes of the last
/// good ingest while Holon is off. The stored hash still lags the disk, so
/// the boot ingests the file again rather than taking the hash skip.
fn reverted_while_off() -> Vec<Step> {
    vec![
        Step::Edit(0, Bytes::Variant(2)),
        Step::FailIngests(0, true),
        Step::Edit(0, Bytes::Variant(1)),
        Step::Restart(vec![(0, Bytes::Revert)]),
    ]
}

/// The next boot's ingest of an offline edit fails, and the user puts back
/// the bytes of the last good ingest. One clean restart leaves the stored hash
/// behind the disk, so the boot after that ingests the file again.
fn reverted_after_a_failed_boot() -> Vec<Step> {
    vec![
        Step::Restart(Vec::new()),
        Step::FailIngests(0, true),
        Step::Restart(vec![(0, Bytes::Variant(1))]),
        Step::Restart(vec![(0, Bytes::Revert)]),
    ]
}

/// Putting back the bytes already on disk while ingests fail owes no ingest,
/// so nothing is disclosed.
fn unchanged_bytes_while_failing() -> Vec<Step> {
    vec![Step::FailIngests(1, true), Step::Edit(1, Bytes::Revert)]
}

/// Two clean restarts let the stored hash catch up with the disk, so the boot
/// after a revert while off takes the hash skip for the failed file.
fn reverted_once_the_skip_is_armed() -> Vec<Step> {
    vec![
        Step::Restart(Vec::new()),
        Step::Restart(Vec::new()),
        Step::FailIngests(0, true),
        Step::Edit(0, Bytes::Variant(1)),
        Step::Restart(vec![(0, Bytes::Revert)]),
    ]
}

/// Steps that start after the boot skip is armed for every file.
fn armed_steps() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(step(), 1..6).prop_map(|steps| {
        let mut armed = vec![Step::Restart(Vec::new()), Step::Restart(Vec::new())];
        armed.extend(steps);
        armed
    })
}

#[test]
fn a_failed_ingest_stays_disclosed_until_the_file_ingests() {
    init_tracing();
    let mut failures: Vec<String> = [
        reverted_while_off(),
        reverted_after_a_failed_boot(),
        unchanged_bytes_while_failing(),
        reverted_once_the_skip_is_armed(),
    ]
    .into_iter()
    .filter_map(|steps| check(&steps).err().map(|e| format!("{steps:?}: {e}")))
    .collect();
    if let Err(e) = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 8,
        failure_persistence: None,
        ..ProptestConfig::default()
    })
    .run(
        &prop_oneof![prop::collection::vec(step(), 1..6), armed_steps()],
        |steps| check(&steps),
    ) {
        failures.push(format!("generated: {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
