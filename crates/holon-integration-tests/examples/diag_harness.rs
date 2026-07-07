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
            "* Block {i}: representative headline text with a [[link]] and *emphasis*\n\
             :PROPERTIES:\n:ID: soak-{i}\n:END:\nSome body paragraph for block {i}.\n\n"
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
        eprintln!("[diag] tokio-console subscriber spawned (bind TOKIO_CONSOLE_BIND, default 127.0.0.1:6669)");
    }

    let seed = env_usize("HOLON_SOAK_SEED_BLOCKS", 500);
    let hold_secs = env_usize("HOLON_DIAG_HOLD_SECS", 0) as u64;

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    let rt2 = rt.clone();
    rt.block_on(async move {
        eprintln!("[diag] seeding {seed} blocks via headless engine boot…");
        let org = build_org(seed);
        let env = TestEnvironmentBuilder::new()
            .with_org_file("soak.org", org)
            .build(rt2.clone())
            .await?;

        // Prove the full ingest pipeline ran end-to-end: the last block must
        // have projected into the SQL read model.
        let last = format!("soak-{}", seed - 1);
        let ok = env.wait_for_block(&last, Duration::from_secs(180)).await;
        anyhow::ensure!(ok, "last block {last} never projected within 180s");

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
