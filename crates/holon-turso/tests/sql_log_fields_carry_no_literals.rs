//! Contract: no `DbHandle` span field and no `holon_turso` log line carries a
//! literal out of the statement it describes.
//!
//! A statement reaches the database with its values inline whenever the caller
//! built the text instead of binding parameters — `BackendEngine` does exactly
//! that for every watched query (`inline_parameters`). The span fields are
//! exported: stdout, `HOLON_LOG=file://…`, and the OTLP span exporter. So a
//! literal recorded on a span leaves the machine with the trace.
//!
//! The statement SHAPE must survive: a redaction that blanked the SQL code as
//! well would pass the first half of this test and make every span useless.

use std::sync::Arc;
use std::sync::Mutex;

use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Values a reader of the log must never see. Each is distinctive enough that a
/// substring search cannot match the surrounding SQL code by accident.
const SECRET_BODY: &str = "SUPERSECRETNOTEBODY";
const SECRET_DEFAULT: &str = "SUPERSECRETCOLUMNDEFAULT";
const SECRET_AMOUNT: &str = "991337";
const SECRET_DDL_NUMBER: &str = "424242";

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log poisoned").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("log poisoned").clone()).expect("utf-8 log")
    }
}

/// `FmtSpan::NEW` emits span creation, which is where `#[tracing::instrument]`
/// records its `sql` field. `trace` lets the actor's own statement traces
/// through too, so one capture covers both.
///
/// Scoped to `holon_turso`: `turso_core` logs the statement AND each string
/// value of its program at DEBUG (`Preparing:`, `String8 { value: … }`). That
/// exposure belongs to the engine fork, and the production filter
/// (`holon_gpui=info,holon=info,holon_tui=info`) leaves `turso_core` at the
/// default ERROR, so only an explicit `RUST_LOG` turns it on.
fn capture() -> Log {
    let log = Log::default();
    let writer = log.clone();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(move || writer.clone())
                .with_ansi(false)
                .with_span_events(FmtSpan::NEW)
                .with_filter(EnvFilter::new("holon_turso=trace")),
        )
        // Each nextest test owns its process, so this is the only subscriber
        // there. Under `cargo test` a second call loses the race and reads an
        // empty log — the callsites stay enabled, which is all the panic test
        // needs.
        .try_init()
        .ok();
    log
}

async fn open() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    // The actor lives as long as a handle does; the backend only owns the
    // database file, which an in-memory database does not have.
    std::mem::forget(backend);
    handle
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_dbhandle_span_or_log_line_carries_a_sql_literal() {
    let log = capture();
    let handle = open().await;

    handle
        .execute_ddl(&format!(
            "CREATE TABLE note (id TEXT PRIMARY KEY, body TEXT, amount INTEGER DEFAULT \
             {SECRET_DDL_NUMBER})"
        ))
        .await
        .expect("create note");

    handle
        .execute_ddl_with_deps(
            &format!(
                "CREATE TABLE tag (id TEXT PRIMARY KEY, label TEXT DEFAULT '{SECRET_DEFAULT}')"
            ),
            vec![],
            vec![],
            0,
        )
        .await
        .expect("create tag");

    handle
        .execute_values(
            &format!(
                "INSERT INTO note (id, body, amount) VALUES ('n1', '{SECRET_BODY}', \
                 {SECRET_AMOUNT})"
            ),
            vec![],
        )
        .await
        .expect("insert note");

    let rows = handle
        .query(
            &format!(
                "SELECT id FROM note WHERE body = '{SECRET_BODY}' AND amount = {SECRET_AMOUNT}"
            ),
            Default::default(),
        )
        .await
        .expect("select note");
    assert_eq!(rows.len(), 1, "the statements under test must really run");

    let text = log.text();
    assert!(
        !text.is_empty(),
        "the capture recorded nothing, so this test proves nothing"
    );
    for secret in [
        SECRET_BODY,
        SECRET_DEFAULT,
        SECRET_AMOUNT,
        SECRET_DDL_NUMBER,
    ] {
        assert!(
            !text.contains(secret),
            "the literal {secret} reached the log:\n{}",
            lines_with(&text, secret)
        );
    }

    // The shape a span is recorded FOR must still be readable.
    for shape in [
        "SELECT id FROM note WHERE body =",
        "INSERT INTO note (id, body, amount) VALUES",
        "CREATE TABLE note (id TEXT PRIMARY KEY, body TEXT, amount INTEGER DEFAULT",
    ] {
        assert!(
            text.contains(shape),
            "redaction erased the statement shape {shape:?}; the spans are now useless:\n{text}"
        );
    }
}

/// A multi-byte character astride the 120-byte truncation boundary of the
/// actor's statement trace. Slicing a `&str` by byte index there panics, and
/// user content is what puts the character at that offset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_multibyte_character_at_the_trace_boundary_does_not_panic() {
    let _log = capture();
    let handle = open().await;
    handle
        .execute_ddl("CREATE TABLE wide (id TEXT PRIMARY KEY, body TEXT)")
        .await
        .expect("create wide");

    // Pad so the first byte of the two-byte 'ä' lands on byte 119.
    let prefix = "x".repeat(119 - "SELECT id FROM wide WHERE body = '".len());
    let sql = format!(
        "SELECT id FROM wide WHERE body = '{prefix}ä{}'",
        "y".repeat(40)
    );
    assert!(
        !sql.is_char_boundary(120),
        "this test needs a character astride byte 120"
    );

    handle
        .query(&sql, Default::default())
        .await
        .expect("the trace of a statement with a wide character must not panic");
}

fn lines_with(text: &str, needle: &str) -> String {
    text.lines()
        .filter(|l| l.contains(needle))
        .collect::<Vec<_>>()
        .join("\n")
}
