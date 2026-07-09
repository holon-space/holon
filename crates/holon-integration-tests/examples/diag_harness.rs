//! Headless diagnostic harness for the two perf-diagnostics tools.
//!
//! It boots a *real* headless Holon session (org parse -> Loro -> CDC ->
//! Turso, including the async `DatabaseActor`), ingests a synthetic vault of
//! `HOLON_SOAK_SEED_BLOCKS` blocks, forces a projection query, then exits.
//! This is deliberately the same engine boot the integration tests drive, so
//! the allocation and task-scheduling behaviour is production-faithful — the
//! GPUI window is the only thing missing, and it cannot run headlessly here.
//!
//! ## Heap profiling (dhat)
//! Enabling `heap-profile` turns on holon-frontend's dhat `#[global_allocator]`
//! and this harness starts/stops the profiler, so `dhat-heap.json` is written
//! on a clean exit (or on Ctrl+C).
//!
//! ```text
//! HOLON_SOAK_SEED_BLOCKS=2000 \
//!   cargo run --release --example diag_harness \
//!   --features heap-profile
//! # -> writes dhat-heap.json in the cwd
//! ```
//!
//! ## Cold-boot ingest benchmark (boot ingest latency, Options 0+1)
//! Set `HOLON_SOAK_SEED_FILES` > 1 to seed a MANY-FILE vault (M files ×
//! `HOLON_SOAK_BLOCKS_PER_FILE` blocks each) instead of one big file. Boot
//! ingest is `N_files × (parse + write + feed barrier)`, so only a many-file
//! vault exercises the per-file cadence. `TestEnvironmentBuilder` builds a
//! fresh empty Turso per run (no persisted `file.content_hash` rows), so every
//! file is ingested — the run is **cold by construction** (the warm-boot hash
//! fast-path cannot engage). The harness prints boot-to-last-block wall time;
//! run under `RUST_LOG=holon_latency=debug` and pipe to
//! `scripts/measure_latency.py` for the per-phase (`boot_parse`/`boot_write`/
//! `boot_feed_wait`/`boot_feed_converge`) split.
//!
//! ```text
//! HOLON_SOAK_SEED_FILES=200 HOLON_SOAK_BLOCKS_PER_FILE=10 \
//!   RUST_LOG=holon_latency=debug \
//!   cargo run --release --example diag_harness -p holon-integration-tests \
//!   --features boot-bench 2>&1 | tee /tmp/boot.log
//! python3 scripts/measure_latency.py /tmp/boot.log
//! ```
//!
//! ## Async-stall profiling (tokio-console)
//! Enabling `tokio-console` (and building with `--cfg tokio_unstable`) starts a
//! `console_subscriber` gRPC aggregator so the `tokio-console` CLI can attach
//! and show per-task poll/idle times. Use `HOLON_DIAG_HOLD_SECS` to keep the
//! process alive long enough to attach.
//!
//! ```text
//! RUSTFLAGS="--cfg tokio_unstable" HOLON_DIAG_HOLD_SECS=120 \
//!   cargo run --example diag_harness \
//!   --features tokio-console
//! # then, in another shell:  tokio-console http://127.0.0.1:6669
//! ```

use std::sync::Arc;
use std::time::Duration;

use holon_api::QueryLanguage;
use holon_integration_tests::TestEnvironmentBuilder;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn build_org(n: usize) -> String {
    let mut s = String::with_capacity(n * 96);
    for i in 0..n {
        s.push_str(&format!(
            "* Block {i}: representative headline text with a [[link]] and \
             *emphasis*\n:PROPERTIES:\n:ID: soak-{i}\n:END:\nSome body paragraph for block \
             {i}.\n\n"
        ));
    }
    s
}

/// One file's org content with a stable `#+ID:` and stable per-block `:ID:`s,
/// so re-render produces identical bytes (no `#+ID:` writeback churn to skew
/// timing).
fn build_org_file(file_idx: usize, blocks: usize) -> String {
    let mut s = format!("#+ID: file-{file_idx}\n#+TITLE: Page {file_idx}\n\n");
    for j in 0..blocks {
        s.push_str(&format!(
            "* Block {file_idx}-{j}: headline with a [[link]] and *emphasis*\n:PROPERTIES:\n:ID: \
             p{file_idx}_{j}\n:END:\nBody paragraph {file_idx}-{j}.\n\n"
        ));
    }
    s
}

fn main() -> anyhow::Result<()> {
    // dhat: dropping this guard at the end of main writes dhat-heap.json.
    // The global allocator itself is installed by holon-frontend when the
    // `heap-profile` feature is on, so it covers the whole process.
    #[cfg(feature = "heap-profile")]
    let _heap_guard = holon_frontend::memory_monitor::heap_profile::start();

    // boot-bench: init a RUST_LOG fmt subscriber (to stderr) so the cold-boot
    // `boot_*` holon_latency events are captured for `measure_latency.py`.
    // Skipped under tokio-console, which installs its own registry below.
    #[cfg(all(feature = "boot-bench", not(feature = "tokio-console")))]
    {
        use tracing_subscriber::EnvFilter;
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }

    // tokio-console: expose task poll-times on the gRPC port (default
    // 127.0.0.1:6669). Only records real data under `--cfg tokio_unstable`.
    #[cfg(feature = "tokio-console")]
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let console = console_subscriber::ConsoleLayer::builder()
            .with_default_env()
            .spawn();
        tracing_subscriber::registry().with(console).init();
        eprintln!(
            "[diag] tokio-console subscriber spawned (bind TOKIO_CONSOLE_BIND, default \
             127.0.0.1:6669)"
        );
    }

    let seed = env_usize("HOLON_SOAK_SEED_BLOCKS", 500);
    let hold_secs = env_usize("HOLON_DIAG_HOLD_SECS", 0) as u64;

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    // Cold-boot many-file benchmark when HOLON_SOAK_SEED_FILES > 1.
    let seed_files = env_usize("HOLON_SOAK_SEED_FILES", 1);
    let blocks_per_file = env_usize("HOLON_SOAK_BLOCKS_PER_FILE", 10);

    let rt2 = rt.clone();
    rt.block_on(async move {
        let mut builder = TestEnvironmentBuilder::new();
        let last_block: String;
        if seed_files > 1 {
            eprintln!(
                "[diag] cold-boot many-file bench: {seed_files} files × {blocks_per_file} blocks \
                 (empty Turso by construction)…"
            );
            for i in 0..seed_files {
                builder = builder
                    .with_org_file(format!("page-{i}.org"), build_org_file(i, blocks_per_file));
            }
            last_block = format!("p{}_{}", seed_files - 1, blocks_per_file - 1);
        } else {
            eprintln!("[diag] seeding {seed} blocks via headless engine boot…");
            builder = builder.with_org_file("soak.org", build_org(seed));
            last_block = format!("soak-{}", seed - 1);
        }

        // Time boot-to-last-block: build() spawns the initial scan; the last
        // block projecting into the SQL read model marks ingest complete.
        let t_boot = std::time::Instant::now();
        let env = builder.build(rt2.clone()).await?;
        let ok = env
            .wait_for_block(&last_block, Duration::from_secs(180))
            .await;
        anyhow::ensure!(ok, "last block {last_block} never projected within 180s");
        eprintln!(
            "[diag] BOOT-TO-PAGES-COMPLETE: {} ms  ({} files, {} blocks/file)",
            t_boot.elapsed().as_millis(),
            if seed_files > 1 { seed_files } else { 1 },
            if seed_files > 1 {
                blocks_per_file
            } else {
                seed
            },
        );

        // Exercise the read path.
        let rows = env
            .query(
                "from block | select {id, content}",
                QueryLanguage::HolonPrql,
            )
            .await?;
        eprintln!("[diag] ingest complete — projected {} blocks", rows.len());

        if hold_secs > 0 {
            eprintln!("[diag] holding {hold_secs}s so tokio-console can attach…");
            tokio::time::sleep(Duration::from_secs(hold_secs)).await;
        }
        Ok::<_, anyhow::Error>(())
    })?;

    eprintln!("[diag] done — dropping dhat guard flushes dhat-heap.json (if heap-profile)");
    Ok(())
}
