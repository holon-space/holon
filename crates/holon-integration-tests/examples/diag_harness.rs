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
//! ## One-file cold boot + boot budget
//! `HOLON_SOAK_ONE_FILE_BLOCKS=N` seeds a SINGLE org file of N blocks nested
//! `HOLON_SOAK_ONE_FILE_BRANCHING` (default 8) wide — the shape of a real
//! vault's dominant file, and the only one whose boot is dominated by
//! INTRA-file work. Two opt-in budgets fail the run when exceeded (both `0` =
//! off, so a plain diagnostic run is never a timing gate):
//! `HOLON_SOAK_BOOT_BUDGET_MS` on wall time, and
//! `HOLON_SOAK_MAX_CHILDREN_READS` on the ingest's per-parent ordering reads
//! (`holon_filesystem::ingest_progress`) — a load-independent observable.
//!
//! ```text
//! HOLON_SOAK_ONE_FILE_BLOCKS=16000 HOLON_SOAK_BOOT_BUDGET_MS=60000 \
//!   cargo run --release --example diag_harness -p holon-integration-tests \
//!   --features boot-bench 2>&1 | tee /tmp/onefile.log
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

/// ONE file of `blocks` headlines nested `branching`-wide: every block gets a
/// parent that is a real headline, so the file has ~`blocks / branching`
/// DISTINCT parents. The flat [`build_org_file`] shape has exactly one parent
/// and therefore cannot exercise the per-parent ingest reads at all — the real
/// vault's dominant file is nested, and that is what the boot budget is about.
fn build_one_file_nested(blocks: usize, branching: usize) -> String {
    let mut s = String::from("#+ID: one-file\n#+TITLE: One Big Page\n\n");
    for i in 0..blocks {
        // Level 1 opens a new parent every `branching` blocks; the rest are its
        // level-2 children.
        let stars = if i % branching == 0 { "*" } else { "**" };
        s.push_str(&format!(
            "{stars} Block {i}: headline with a [[link]] and *emphasis*\n:PROPERTIES:\n:ID: \
             one{i}\n:END:\nBody paragraph {i}.\n\n"
        ));
    }
    s
}

/// The real vault's file-size distribution (1,001 files, 40,989 headlines,
/// measured 2026-07-28): 274 empty, 210 with 1-4, 272 with 5-19, 225 with
/// 20-99, 19 with 100-999 headlines, and ONE file with 24,084. The single
/// huge file is what exercises the O(K²) sibling re-read term, so a corpus
/// of uniform small files cannot reproduce the prod cost curve.
const VAULT_BUCKETS: [(usize, usize, usize); 5] = [
    (274, 0, 0),
    (210, 1, 4),
    (272, 5, 19),
    (225, 20, 99),
    (19, 100, 999),
];

/// Block count for the `i`-th non-huge file of a vault-shaped corpus of
/// `files` files, drawn deterministically from [`VAULT_BUCKETS`].
fn vault_shape_blocks(i: usize, files: usize) -> usize {
    let total: usize = VAULT_BUCKETS.iter().map(|(n, _, _)| n).sum();
    let scaled = i * total / files.max(1);
    let mut acc = 0;
    for (n, lo, hi) in VAULT_BUCKETS {
        acc += n;
        if scaled < acc {
            return if hi == 0 {
                0
            } else {
                lo + (i * 7919) % (hi - lo + 1)
            };
        }
    }
    0
}

/// Vault-shaped corpus: `files` files whose sizes follow the real vault's
/// distribution, one of them carrying `big` blocks, spread over a few
/// directory levels like the real vault. Never emits an `X.org` next to a
/// directory `X/` — that folder-companion shape is a separate (identity)
/// concern and would confound the timing.
fn build_vault_corpus(files: usize, big: usize) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(files);
    out.push(("big/Dominant.org".to_string(), build_org_file(0, big)));
    for i in 1..files {
        let blocks = vault_shape_blocks(i, files);
        let name = match i % 4 {
            0 => format!("page-{i}.org"),
            1 => format!("area/page-{i}.org"),
            2 => format!("area/sub/page-{i}.org"),
            _ => format!("proj/page-{i}.org"),
        };
        out.push((name, build_org_file(i, blocks)));
    }
    out
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
    // Vault-shaped corpus (real distribution + one dominant file).
    let vault_files = env_usize("HOLON_SOAK_VAULT_FILES", 0);
    let vault_big = env_usize("HOLON_SOAK_VAULT_BIG", 2000);
    // Prod-faithful boot: GPUI runs with `wait_for_ready=false` AND resolves
    // `LoroSyncControllerHandle` right after bootstrap, so the projector run
    // loop reconciles CONCURRENTLY with the org initial scan — one pass per
    // Loro commit. The default fixture boot waits for the scan first, so the
    // run loop never sees the scan's commits and the per-op cadence is absent.
    let prod_boot = env_usize("HOLON_SOAK_PROD_BOOT", 0) != 0;
    let wait_secs = env_usize("HOLON_SOAK_WAIT_SECS", 180) as u64;
    // ONE file × N blocks — the shape a real vault's dominant file has, and the
    // only one whose cold boot is dominated by INTRA-file work.
    let one_file_blocks = env_usize("HOLON_SOAK_ONE_FILE_BLOCKS", 0);
    let one_file_branching = env_usize("HOLON_SOAK_ONE_FILE_BRANCHING", 8);
    // Boot budget + ingest-read budget. Both opt-in (0 = off) so a CI run that
    // just wants the harness output is never made flaky by a timing gate.
    let budget_ms = env_usize("HOLON_SOAK_BOOT_BUDGET_MS", 0) as u128;
    let max_children_reads = env_usize("HOLON_SOAK_MAX_CHILDREN_READS", 0) as u64;
    let max_create_commits = env_usize("HOLON_SOAK_MAX_CREATE_COMMITS", 0) as u64;

    let rt2 = rt.clone();
    rt.block_on(async move {
        let mut builder = TestEnvironmentBuilder::new().wait_for_file_watcher(!prod_boot);
        let last_block: String;
        // Set for the vault-shaped corpus: scan order is not file order, so
        // completion is a COUNT watermark, not one nominated block.
        let mut expect_blocks = 0usize;
        if one_file_blocks > 0 {
            eprintln!(
                "[diag] ONE-file cold boot: {one_file_blocks} blocks, branching \
                 {one_file_branching}, prod_boot={prod_boot}…"
            );
            builder = builder.with_org_file(
                "big/OneFile.org".to_string(),
                build_one_file_nested(one_file_blocks, one_file_branching),
            );
            expect_blocks = one_file_blocks;
            last_block = String::new();
        } else if vault_files > 1 {
            eprintln!(
                "[diag] vault-shaped cold boot: {vault_files} files, dominant file \
                 {vault_big} blocks, prod_boot={prod_boot}…"
            );
            let corpus = build_vault_corpus(vault_files, vault_big);
            for (i, (name, content)) in corpus.into_iter().enumerate() {
                expect_blocks += if i == 0 {
                    vault_big
                } else {
                    vault_shape_blocks(i, vault_files)
                };
                builder = builder.with_org_file(name, content);
            }
            eprintln!("[diag] corpus: {expect_blocks} headline block(s)");
            last_block = String::new();
        } else if seed_files > 1 {
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
        // Prod parity: GPUI resolves `LoroSyncControllerHandle` immediately
        // after bootstrap (its MCP debug-handles cell, `frontends/gpui/src/
        // main.rs`), which STARTS the projector run loop while the org initial
        // scan is still running. `TestEnvironmentBuilder` only `try_resolve`s
        // it synchronously, which misses the async provider unless something
        // already awaited it — so without this the fixture boots with the run
        // loop dead and cannot reproduce the per-commit cadence.
        let _prod_run_loop = if prod_boot {
            let injector = env
                .injector()
                .ok_or_else(|| anyhow::anyhow!("no injector on TestEnvironment"))?;
            Some(
                injector
                    .try_resolve_async::<holon_loro::LoroSyncControllerHandle>()
                    .await
                    .map_err(|e| anyhow::anyhow!("resolve LoroSyncControllerHandle: {e}"))?,
            )
        } else {
            None
        };
        eprintln!(
            "[diag] boot returned at {} ms, run_loop_live={}",
            t_boot.elapsed().as_millis(),
            _prod_run_loop.is_some(),
        );
        if expect_blocks > 0 {
            let deadline = std::time::Instant::now() + Duration::from_secs(wait_secs);
            let mut next_sample = std::time::Instant::now();
            loop {
                let n = env
                    .query("from block | select {id}", QueryLanguage::HolonPrql)
                    .await?
                    .len();
                if std::time::Instant::now() >= next_sample {
                    let s = holon_loro::projection_stats::snapshot();
                    eprintln!(
                        "[diag] t={}ms blocks={} passes={} ops={}",
                        t_boot.elapsed().as_millis(),
                        n,
                        s.passes,
                        s.ops
                    );
                    next_sample = std::time::Instant::now() + Duration::from_secs(5);
                }
                if n >= expect_blocks {
                    break;
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "only {n} of {expect_blocks} block(s) projected within {wait_secs}s"
                );
                // 2s, not sub-second: the count query returns every row and
                // contends with the ingest for the DatabaseActor, which would
                // itself perturb the cadence being measured.
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        } else {
            let ok = env
                .wait_for_block(&last_block, Duration::from_secs(wait_secs))
                .await;
            anyhow::ensure!(
                ok,
                "last block {last_block} never projected in {wait_secs}s"
            );
        }
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

        // Ingest read counters + the two opt-in budgets. `children_reads` is
        // the load-independent half of the boot budget: it counts the ingest's
        // per-parent ordering reads, so a regression that re-introduces
        // per-block reads fails here on any machine, not just a slow one.
        let ingest = holon_filesystem::ingest_progress::snapshot();
        let boot_ms = t_boot.elapsed().as_millis();
        eprintln!(
            "[diag] INGEST READS: files={} blocks={} children_reads={} doc_walks={} \
             create_commits={}",
            ingest.files,
            ingest.blocks,
            ingest.children_reads,
            ingest.doc_walks,
            ingest.create_commits,
        );
        if max_create_commits > 0 {
            anyhow::ensure!(
                ingest.create_commits <= max_create_commits,
                "ingest issued {} create commit(s) for {} block(s) — budget is \
                 {max_create_commits}; creates are committing per block",
                ingest.create_commits,
                ingest.blocks,
            );
        }
        if max_children_reads > 0 {
            anyhow::ensure!(
                ingest.children_reads <= max_children_reads,
                "ingest issued {} children read(s) for {} block(s) over {} file(s) — budget is \
                 {max_children_reads}; the per-parent reads are scaling with block count",
                ingest.children_reads,
                ingest.blocks,
                ingest.files,
            );
        }
        if budget_ms > 0 {
            anyhow::ensure!(
                boot_ms <= budget_ms,
                "boot took {boot_ms}ms, over the {budget_ms}ms budget"
            );
        }

        // Cold-boot CADENCE — the parity metric. Prod (real vault, 2026-07-28)
        // ran 16,333 passes for 25,139 ops, 87.2 % of them single-op, because
        // the projector run loop reconciles per Loro commit during the scan.
        let st = holon_loro::projection_stats::snapshot();
        eprintln!(
            "[diag] PROJECTION CADENCE: passes={} ops={} ops/pass={:.2} \
             single_op={:.1}% snapshot_ms={} apply_ms={}",
            st.passes,
            st.ops,
            st.ops as f64 / st.passes.max(1) as f64,
            100.0 * st.single_op_passes as f64 / st.passes.max(1) as f64,
            st.snapshot_ms,
            st.apply_ms,
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
