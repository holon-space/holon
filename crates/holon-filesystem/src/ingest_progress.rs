//! Intra-file ingest liveness: per-N-block INFO lines, a no-progress watchdog
//! that fires *while* one file's ingest is wedged, and the process-wide read
//! counters the boot-budget harness asserts on.
//!
//! The scan-level watchdog (`FileSyncController::finish_initial_scan`) only
//! sees the per-FILE loop, so a single very large org file produced 47 minutes
//! of silence with nothing able to distinguish "slow" from "stuck".

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

/// One INFO line per this many blocks inside a single file's ingest: a
/// 50k-block file logs ~25 lines — enough to show minute-by-minute liveness,
/// few enough to keep a boot log readable.
pub const PROGRESS_EVERY_BLOCKS: usize = 2_000;

/// How often the watchdog task samples the progress counter. Sub-second
/// sampling would only sharpen the reported stall age, which is already
/// reported at `stall` granularity.
const WATCHDOG_TICK: Duration = Duration::from_secs(1);

/// No-progress window for the intra-file watchdog, matching the scan-level
/// window `holon-orgmode`'s DI passes to `finish_initial_scan`.
pub const INTRA_FILE_STALL: Duration = Duration::from_secs(30);

static FILES: AtomicU64 = AtomicU64::new(0);
static BLOCKS: AtomicU64 = AtomicU64::new(0);
static CHILDREN_READS: AtomicU64 = AtomicU64::new(0);
static DOC_WALKS: AtomicU64 = AtomicU64::new(0);
static CREATE_COMMITS: AtomicU64 = AtomicU64::new(0);

/// Process-wide ingest read counters. The boot-budget harness asserts on
/// `children_reads` because a read COUNT is a sturdier regression observable
/// than wall time: it is machine- and load-independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestStats {
    /// Files whose block-ingest phase ran (hash-skipped files excluded).
    pub files: u64,
    /// Blocks walked by the per-file ingest passes.
    pub blocks: u64,
    /// `BlockOrdering::children` reads issued by the ingest replay loop.
    pub children_reads: u64,
    /// Doc-scoped `BlockReader::get_blocks` walks issued by the ingest.
    pub doc_walks: u64,
    /// Create handoffs to the ordering authority — one per BATCH, which is one
    /// authority commit. Per-block creates make this equal the block count.
    pub create_commits: u64,
}

/// Snapshot of the process-wide ingest counters.
pub fn snapshot() -> IngestStats {
    IngestStats {
        files: FILES.load(Ordering::Relaxed),
        blocks: BLOCKS.load(Ordering::Relaxed),
        children_reads: CHILDREN_READS.load(Ordering::Relaxed),
        doc_walks: DOC_WALKS.load(Ordering::Relaxed),
        create_commits: CREATE_COMMITS.load(Ordering::Relaxed),
    }
}

/// Zero the counters so a caller can measure one phase in isolation.
pub fn reset() {
    FILES.store(0, Ordering::Relaxed);
    BLOCKS.store(0, Ordering::Relaxed);
    CHILDREN_READS.store(0, Ordering::Relaxed);
    DOC_WALKS.store(0, Ordering::Relaxed);
    CREATE_COMMITS.store(0, Ordering::Relaxed);
}

/// Count one `BlockOrdering::children` read from the ingest replay loop.
pub(crate) fn record_children_read() {
    CHILDREN_READS.fetch_add(1, Ordering::Relaxed);
}

/// Count one doc-scoped `get_blocks` walk from the ingest.
pub(crate) fn record_doc_walk() {
    DOC_WALKS.fetch_add(1, Ordering::Relaxed);
}

/// Count one create handoff to the ordering authority (one authority commit).
pub(crate) fn record_create_commit() {
    CREATE_COMMITS.fetch_add(1, Ordering::Relaxed);
}

/// Liveness reporter for ONE file's ingest: `advance` emits the periodic INFO
/// line, and a background task warns when the counter stops moving for
/// `stall`. Dropping it stops the watchdog.
pub struct IngestProgress {
    done: Arc<AtomicUsize>,
    total: usize,
    path: String,
    started: Instant,
    reads_at_start: IngestStats,
    watchdog: tokio::task::JoinHandle<()>,
}

impl IngestProgress {
    /// Begin reporting for `path`, whose parse produced `total` blocks.
    pub fn start(path: &Path, total: usize, stall: Duration) -> Self {
        FILES.fetch_add(1, Ordering::Relaxed);
        let done = Arc::new(AtomicUsize::new(0));
        let display = path.display().to_string();
        let watchdog = tokio::spawn({
            let done = done.clone();
            let path = display.clone();
            async move {
                let mut last = 0usize;
                let mut since = Instant::now();
                loop {
                    tokio::time::sleep(WATCHDOG_TICK).await;
                    let now = done.load(Ordering::Relaxed);
                    if now != last {
                        last = now;
                        since = Instant::now();
                        continue;
                    }
                    if since.elapsed() >= stall {
                        tracing::warn!(
                            "[Ingest] NO PROGRESS for {}s inside a SINGLE file: {} of {} \
                             block(s) done in {} — this one file's ingest is stalled",
                            since.elapsed().as_secs(),
                            now,
                            total,
                            path,
                        );
                        since = Instant::now();
                    }
                }
            }
        });
        Self {
            done,
            total,
            path: display,
            started: Instant::now(),
            reads_at_start: snapshot(),
            watchdog,
        }
    }

    /// Restart the per-block count for a new pass over the file's blocks, so
    /// each pass reports `n of total` against the same denominator.
    pub fn begin_phase(&self) {
        self.done.store(0, Ordering::Relaxed);
    }

    /// Record one processed block of `phase`, logging every
    /// [`PROGRESS_EVERY_BLOCKS`]-th one.
    pub fn advance(&self, phase: &str) {
        BLOCKS.fetch_add(1, Ordering::Relaxed);
        let n = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(PROGRESS_EVERY_BLOCKS) {
            tracing::info!(
                "[Ingest] {} — {} of {} block(s) ({})",
                self.path,
                n,
                self.total,
                phase,
            );
        }
    }
}

impl Drop for IngestProgress {
    fn drop(&mut self) {
        self.watchdog.abort();
        let now = snapshot();
        tracing::info!(
            "[Ingest] {} — done: {} block(s) in {}ms, {} children read(s), {} doc walk(s)",
            self.path,
            self.total,
            self.started.elapsed().as_millis(),
            now.children_reads - self.reads_at_start.children_reads,
            now.doc_walks - self.reads_at_start.doc_walks,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tracing_subscriber::layer::Context;
    use tracing_subscriber::layer::Layer;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[derive(Default)]
    struct WarnCollector(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> Layer<S> for WarnCollector {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            if *event.metadata().level() != tracing::Level::WARN {
                return;
            }
            struct Visit<'a>(&'a mut String);
            impl tracing::field::Visit for Visit<'_> {
                fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                    self.0.push_str(&format!("{value:?}"));
                }
            }
            let mut msg = String::new();
            event.record(&mut Visit(&mut msg));
            self.0.lock().unwrap().push(msg);
        }
    }

    /// Every WARN emitted while `body` ran. The watchdog task runs on the same
    /// (current-thread) runtime, so the thread-local dispatcher covers it.
    async fn warns_while<F: std::future::Future<Output = ()>>(body: F) -> Vec<String> {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(WarnCollector(sink.clone())),
        );
        body.await;
        drop(guard);
        let out = sink.lock().unwrap().clone();
        out
    }

    #[tokio::test]
    async fn watchdog_warns_when_one_file_makes_no_progress() {
        let warns = warns_while(async {
            let _p = IngestProgress::start(
                Path::new("/vault/Huge.org"),
                50_000,
                Duration::from_millis(100),
            );
            tokio::time::sleep(Duration::from_millis(2_500)).await;
        })
        .await;
        assert!(
            warns.iter().any(|w| w.contains("NO PROGRESS")),
            "a wedged single-file ingest must warn; got {warns:?}"
        );
    }

    #[tokio::test]
    async fn watchdog_stays_quiet_while_blocks_keep_landing() {
        let warns = warns_while(async {
            let p = IngestProgress::start(
                Path::new("/vault/Huge.org"),
                50_000,
                Duration::from_millis(1_500),
            );
            for _ in 0..25 {
                p.advance("creates");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(
            !warns.iter().any(|w| w.contains("NO PROGRESS")),
            "a slow but LIVE ingest must not warn; got {warns:?}"
        );
    }
}
