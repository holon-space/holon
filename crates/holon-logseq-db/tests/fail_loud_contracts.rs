//! The importer's refusals, exercised against real inputs.
//!
//! `NamespacePage`, `DanglingReference` and `Corrupt` are the arms that decide
//! whether a graph we cannot represent FAILS or is silently mangled, and none
//! of them fires on the healthy fixture — so without these they were untested
//! code guarding the worst outcomes.
//!
//! Method (adapted from the adversarial verifier's): copy the committed
//! fixture to a temp dir and rewrite `kvs` content with LENGTH-PRESERVING
//! string substitutions, so the Transit documents stay byte-valid and only the
//! one fact under test changes. The committed fixture is never written.

use std::path::Path;
use std::path::PathBuf;

use holon_logseq_db::ImportError;
use holon_logseq_db::read_datoms;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

/// Copy the fixture into `dir` and apply a length-preserving rewrite to every
/// `kvs` row. Returns the copy's path and how many rows changed — asserted by
/// callers, so a substitution that silently matches nothing fails the test
/// rather than producing a false green.
async fn mutated_copy(
    dir: &Path,
    rewrite: impl Fn(&str) -> String,
) -> Result<(PathBuf, usize), Box<dyn std::error::Error>> {
    let dst = dir.join("mutated.sqlite");
    std::fs::copy(fixture_path(), &dst)?;

    let db = libsql::Builder::new_local(&dst).build().await?;
    let conn = db.connect()?;
    let mut rows = conn
        .query("SELECT addr, content FROM kvs WHERE addr > 0", ())
        .await?;
    let mut edits: Vec<(i64, String)> = Vec::new();
    while let Some(row) = rows.next().await? {
        let addr: i64 = row.get(0)?;
        let content: String = row.get(1)?;
        let new = rewrite(&content);
        if new != content {
            assert_eq!(
                new.len(),
                content.len(),
                "the rewrite must preserve length, or the document stops being \
                 the one under test"
            );
            edits.push((addr, new));
        }
    }
    let changed = edits.len();
    for (addr, content) in edits {
        conn.execute(
            "UPDATE kvs SET content = ?1 WHERE addr = ?2",
            (content, addr),
        )
        .await?;
    }
    Ok((dst, changed))
}

/// A `/` in a page name needs page-under-page chain construction, which
/// stage 1 does not do. It must REFUSE rather than flatten the slash into one
/// page named `a/b` — a wrong page identity that would look perfectly normal.
#[tokio::test]
async fn a_namespace_page_name_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, changed) = mutated_copy(dir.path(), |c| {
        c.replace("\"project alpha\"", "\"project/alpha\"")
    })
    .await
    .expect("build the mutated copy");
    assert!(changed > 0, "the page-name substitution matched no rows");

    let err = read_datoms(&path)
        .await
        .and_then(|set| holon_logseq_db::project(&set))
        .expect_err("a namespace page must stop the import");
    assert!(
        matches!(&err, ImportError::NamespacePage { name } if name == "project/alpha"),
        "got {err:?}"
    );
}

/// A `:block/parent` pointing at an entity that is not a block must refuse.
/// Dropping the edge instead would silently reparent a whole subtree to the
/// root, which is indistinguishable from a successful import.
#[tokio::test]
async fn a_dangling_parent_reference_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    // 993 is far above the fixture's highest entity id (215), and the same
    // width as 193, so the document length is unchanged.
    let (path, changed) = mutated_copy(dir.path(), |c| {
        c.replace("\"~:block/parent\",193,", "\"~:block/parent\",993,")
    })
    .await
    .expect("build the mutated copy");
    assert!(changed > 0, "the parent substitution matched no rows");

    let err = read_datoms(&path)
        .await
        .and_then(|set| holon_logseq_db::project(&set))
        .expect_err("a dangling parent must stop the import");
    assert!(
        matches!(
            &err,
            ImportError::DanglingReference { attr, to: 993, .. } if attr == ":block/parent"
        ),
        "got {err:?}"
    );
}

/// A file that is not a database must say so. Reporting it as `Locked`
/// ("LogSeq appears to be running") sends the reader to make a snapshot copy,
/// which cannot help, and hides the corruption.
#[tokio::test]
async fn a_file_that_is_not_a_database_is_not_reported_as_locked() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("garbage.sqlite");
    std::fs::write(&path, vec![0x7fu8; 4096]).expect("write the garbage file");

    let err = read_datoms(&path)
        .await
        .expect_err("a non-database must stop the import");
    assert!(
        matches!(err, ImportError::Corrupt { .. }),
        "a non-database must report as Corrupt, not Locked; got {err:?}"
    );
    let message = err.to_string();
    assert!(
        !message.contains("locked") && !message.contains("running"),
        "the message must not blame a running LogSeq: {message:?}"
    );
}

/// A path that does not exist errors rather than being created — the
/// read-only rule, stated as a test.
#[tokio::test]
async fn a_missing_file_is_not_created() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("absent.sqlite");

    let err = read_datoms(&path)
        .await
        .expect_err("a missing file must error");
    assert!(
        matches!(err, ImportError::Open { .. } | ImportError::Corrupt { .. }),
        "got {err:?}"
    );
    assert!(!path.exists(), "the importer must never create a db file");
}

/// A `kvs` row holding a JSON OBJECT must be an error naming the row, on BOTH
/// read paths — never a panic.
///
/// Transit-JSON writes maps as arrays, so an object cannot come from LogSeq;
/// it means the file is corrupt. `kvs` content is bytes on disk that no
/// invariant of ours governs, so this is external input being wrong rather
/// than an assumption of ours breaking, and the fail-loud rule says `Err`.
/// Before W1 this reached an `unreachable!()` and took the process down.
#[tokio::test]
async fn a_row_holding_a_json_object_is_an_error_naming_the_row_not_a_panic() {
    let dir = tempfile::tempdir().expect("temp dir");
    let dst = dir.path().join("object-row.sqlite");
    std::fs::copy(fixture_path(), &dst).expect("copy the fixture");

    // Not length-preserving, so this is a direct write rather than the shared
    // substitution harness. Addr 1000001 is a real tree row.
    const VICTIM: i64 = 1_000_001;
    let db = libsql::Builder::new_local(&dst)
        .build()
        .await
        .expect("open copy");
    let conn = db.connect().expect("connect");
    conn.execute(
        "UPDATE kvs SET content = ?1 WHERE addr = ?2",
        libsql::params![r#"{"keys":[],"addresses":[]}"#, VICTIM],
    )
    .await
    .expect("corrupt one row");
    drop(conn);
    drop(db);

    let err = read_datoms(&dst)
        .await
        .expect_err("a JSON-object row must be refused");
    let text = err.to_string();
    assert!(
        matches!(err, ImportError::Decode { addr, .. } if addr == VICTIM),
        "the error must name the row it came from, got {err:?}"
    );
    assert!(
        text.contains(&VICTIM.to_string()),
        "the message must name addr {VICTIM}: {text}"
    );

    let writer_err = holon_logseq_db::kvs_writer::read_graph(&dst)
        .await
        .expect_err("the writer's read path must refuse it too");
    assert!(
        writer_err.to_string().contains(&VICTIM.to_string()),
        "the writer's error must name addr {VICTIM}: {writer_err}"
    );

    // Printed, not just asserted: the whole point of replacing the panic was
    // diagnosability, and an error less informative than the backtrace it
    // replaced would be a regression that a boolean assertion cannot show.
    println!("importer: {text}");
    println!("writer:   {writer_err}");
}
