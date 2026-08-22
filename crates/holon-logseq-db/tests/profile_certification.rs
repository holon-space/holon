//! Certifies `crates/holon-logseq-db/profile.yaml` against Holon's OWN writer
//! surface for a LogSeq DB graph — Increment 2b.3.
//!
//! ORACLE POSTURE: this test needs NO LogSeq oracle and is `#[ignore]`d for
//! nothing. It does not ask what LogSeq would do; it asks what HOLON's writer
//! carries and what it refuses, through `kvs_writer::push` and a real
//! re-import. The oracle legs (`just lsqdb-oracle`) answer the other question —
//! whether our bytes match LogSeq's own transactor — and stay where they are.
//!
//! The write leg is title/content ONLY. Everything else a `BaseDiff` can carry
//! is refused BY NAME (`kvs_writer.rs:1198-1232`), and a refusal is the law's
//! honest branch — the profile has to say so rather than let a caller discover
//! it by losing an edit.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use holon_api::Value;
use holon_capability::CapabilityProfile;
use holon_capability::Carrier;
use holon_capability::CertifiableFormat;
use holon_capability::Leg;
use holon_capability::Readback;
use holon_capability::WriteAttempt;
use holon_capability::WriteLeg;
use holon_capability::certify;
use holon_logseq_db::LogseqDbImporter;
use holon_logseq_db::base::BaseBlock;
use holon_logseq_db::base::ImportBase;
use holon_logseq_db::kvs_writer;
use holon_logseq_db::kvs_writer::KvsGraph;
use tokio::runtime::Handle;

/// The one carrier: a tail transaction appended by `push`, read back by a real
/// re-import of the written file.
const TAIL_LEG: Carrier = Carrier {
    leg: Leg("kvs_tail_transaction"),
    description: "datoms push appends to the kvs tail, observed through a re-import",
};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

struct LogseqDb {
    profile: CapabilityProfile,
}

impl LogseqDb {
    fn load() -> anyhow::Result<Self> {
        let path = std::env::var_os("HOLON_CAPABILITY_PROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("profile.yaml"));
        Ok(Self {
            profile: CapabilityProfile::from_path(&path)
                .context("the logseq-db profile must load")?,
        })
    }

    /// Bridge the SYNC certifier onto the async import/write path.
    fn blocking<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        tokio::task::block_in_place(|| Handle::current().block_on(fut))
    }

    /// A graph plus the base it currently holds, and one block a push may
    /// legitimately target.
    async fn scene(&self) -> anyhow::Result<(KvsGraph, ImportBase, String)> {
        let graph = kvs_writer::read_graph(&fixture())
            .await
            .map_err(|e| anyhow::anyhow!("the fixture graph must read: {e}"))?;
        let importer = LogseqDbImporter::new();
        let base = ImportBase::from_import(
            &importer
                .import(&fixture())
                .await
                .map_err(|e| anyhow::anyhow!("the fixture must import: {e}"))?,
        );
        // The first editable, non-built-in block carrying a title. A push at a
        // built-in page is refused for a DIFFERENT reason, which would make
        // every probe read like a scope refusal.
        let target = base
            .uuids()
            .filter(|u| {
                let block = base.get(u).expect("present");
                !block.content.is_empty() && !block.parent_id.starts_with("sentinel:")
            })
            .find(|u| {
                kvs_writer::entity_by_uuid(&graph, u)
                    .ok()
                    .flatten()
                    .is_some_and(|e| !kvs_writer::is_built_in(&graph, e).unwrap_or(true))
            })
            .map(str::to_string)
            .context("the fixture must carry at least one editable block")?;
        Ok((graph, base, target))
    }
}

impl CertifiableFormat for LogseqDb {
    fn profile(&self) -> &CapabilityProfile {
        &self.profile
    }

    fn carriers(&self) -> &'static [Carrier] {
        &[TAIL_LEG]
    }

    /// A property write, through the REAL push path.
    ///
    /// `push` refuses a property change by name, so this reports `Refused` —
    /// the law's honest branch — rather than a loss. That refusal IS the
    /// clause: a profile claiming a value kind survives a write is wrong here,
    /// and the run says so.
    fn round_trip_property(
        &self,
        _: Carrier,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        self.blocking(async {
            let (mut graph, before, target) = self.scene().await?;
            let mut block = before.get(&target).expect("present").clone();
            block.properties.insert(key.to_string(), value.clone());
            let mut after = before.clone();
            after
                .advance(&target, block)
                .map_err(|e| anyhow::anyhow!("advancing the base must work: {e}"))?;

            match kvs_writer::push(&mut graph, &before, &after) {
                Err(e) => Ok(Readback::Refused {
                    reason: e.to_string(),
                }),
                Ok(_) => {
                    // Not expected today; if push ever carries properties, the
                    // probe must read the value BACK rather than assume.
                    let dir = tempfile::tempdir().context("temp dir")?;
                    let copy = dir.path().join("certified.sqlite");
                    kvs_writer::write_graph(&graph, &copy)
                        .await
                        .map_err(|e| anyhow::anyhow!("writing the graph must work: {e}"))?;
                    let importer = LogseqDbImporter::new();
                    let back = ImportBase::from_import(
                        &importer
                            .import(&copy)
                            .await
                            .map_err(|e| anyhow::anyhow!("re-import must work: {e}"))?,
                    );
                    Ok(
                        match back.get(&target).and_then(|b| b.properties.get(key)) {
                            None => Readback::Absent,
                            Some(found) => Readback::Present(found.clone()),
                        },
                    )
                }
            }
        })
    }

    /// The write leg, DRIVEN: a real title push, written to a file,
    /// re-imported.
    ///
    /// Asked of the writer, never of the profile — comparing the profile with
    /// `supports()` would compare it with itself.
    fn attempt_write(&self) -> anyhow::Result<Option<WriteAttempt>> {
        self.blocking(async {
            let (mut graph, before, target) = self.scene().await?;
            let wanted = "certified title";
            let mut after = before.clone();
            let block = BaseBlock {
                content: wanted.to_string(),
                ..before.get(&target).expect("present").clone()
            };
            after
                .advance(&target, block)
                .map_err(|e| anyhow::anyhow!("advancing the base must work: {e}"))?;

            if let Err(e) = kvs_writer::push(&mut graph, &before, &after) {
                return Ok(Some(WriteAttempt::Refused {
                    reason: e.to_string(),
                }));
            }
            let dir = tempfile::tempdir().context("temp dir")?;
            let copy = dir.path().join("certified.sqlite");
            kvs_writer::write_graph(&graph, &copy)
                .await
                .map_err(|e| anyhow::anyhow!("writing the graph must work: {e}"))?;
            let importer = LogseqDbImporter::new();
            let back = ImportBase::from_import(
                &importer
                    .import(&copy)
                    .await
                    .map_err(|e| anyhow::anyhow!("re-import must work: {e}"))?,
            );
            let carried = back.get(&target).is_some_and(|b| b.content == wanted);
            anyhow::ensure!(
                carried,
                "push reported success but the re-import does not carry the pushed title — the \
                 harness is measuring something other than the write path"
            );
            Ok(Some(WriteAttempt::Wrote {
                leg: WriteLeg::File,
            }))
        })
    }
}

/// Every restriction the logseq-db profile declares is REAL, measured against
/// Holon's writer.
#[tokio::test(flavor = "multi_thread")]
async fn the_logseq_db_profile_declares_only_restrictions_that_are_real() -> anyhow::Result<()> {
    let format = LogseqDb::load()?;
    let report = certify(&format).context("the certification harness must run")?;

    println!("{}", report.render());

    let dir = std::env::var_os("HOLON_CAPABILITY_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/capability-certification")
        });
    let written = report.write_report(format.profile().id(), &dir)?;
    println!("report: {}", written.display());

    assert!(
        report.confirmed > 0,
        "a run that generated NOTHING must not pass as clean:\n{}",
        report.render()
    );
    assert!(
        report.is_clean(),
        "the logseq-db profile declares {} restriction(s) the writer does not honour, and {} \
         coverage gap(s):\n{}",
        report.violations.len(),
        report.gaps.len(),
        report.render()
    );
    Ok(())
}

/// The markers' REASON, pinned.
///
/// Twenty-nine clauses in this profile are excused with "the write boundary is
/// CLOSED to property changes". That reason is a measurement, and a
/// measurement that nothing re-checks is exactly the stale citation this whole
/// increment exists to stop: the day `push` learns to write properties, those
/// markers become lies and the certification would stay green, because a
/// closed boundary drives nothing.
///
/// This is the alarm. It fails when the boundary OPENS.
#[tokio::test(flavor = "multi_thread")]
async fn the_write_boundary_is_still_closed_to_property_changes() -> anyhow::Result<()> {
    let format = LogseqDb::load()?;
    let readback = format.round_trip_property(
        TAIL_LEG,
        "CertificationProbe",
        &Value::String("carried".into()),
    )?;
    match readback {
        Readback::Refused { reason } => {
            assert!(
                reason.contains("property change"),
                "the refusal must still be the SCOPE refusal the markers cite; got: {reason}"
            );
            Ok(())
        }
        other => panic!(
            "the write boundary has OPENED to property changes ({other:?}) — every \
             `not_yet_certified` marker citing the closed boundary is now stale and the axis-3 \
             and axis-4 clauses must be driven and declared for real"
        ),
    }
}

/// The write leg is DRIVEN, and it names the mechanism.
///
/// `confirmed > 0` in the run above rests on this one probe, so it gets an
/// assertion of its own rather than living only as a counter.
#[tokio::test(flavor = "multi_thread")]
async fn a_title_push_reaches_a_re_imported_graph() -> anyhow::Result<()> {
    let format = LogseqDb::load()?;
    let attempt = format
        .attempt_write()?
        .expect("the write probe is driveable");
    assert_eq!(
        attempt,
        WriteAttempt::Wrote {
            leg: WriteLeg::File
        },
        "a title push must reach the FILE and come back through a re-import"
    );
    Ok(())
}

/// The cross-format price tag the draft named, between the two REAL yamls.
///
/// org's `_`-prefixed keys are erased by its flat write leg; LogSeq's graph
/// carries `_logseq_raw/` attributes the importer preserves. This is the
/// promotion story's cost, stated by the profiles rather than by prose.
#[test]
fn moving_between_org_and_logseq_db_has_a_price() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let logseq = CapabilityProfile::from_path(root.join("crates/holon-logseq-db/profile.yaml"))
        .expect("the logseq-db profile loads");
    let org = CapabilityProfile::from_path(root.join("crates/holon-org-format/profile.yaml"))
        .expect("the org profile loads");

    // org → logseq-db: the write leg refuses structure org carries freely.
    let to_logseq = org.diff(&logseq);
    assert!(
        to_logseq
            .iter()
            .any(|l| l.clause == holon_capability::clause::ClauseId::HierarchyReparent),
        "logseq-db REFUSES a re-parent; moving there costs the ability to move things:\n{to_logseq:#?}"
    );

    // logseq-db → org: org RESERVES the `_` prefix, so a key carrying it stops
    // being the author's. A target that declares MORE is still a loss.
    let to_org = logseq.diff(&org);
    assert!(
        to_org
            .iter()
            .any(|l| l.clause == holon_capability::clause::ClauseId::PropertyKeysReservedPrefixes),
        "org reserves `_`, which a LogSeq graph does not:\n{to_org:#?}"
    );
}
