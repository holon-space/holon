//! CRDT-config projection latency benchmark.
//!
//! Boots a real headless Holon session (org -> Loro -> CDC -> Turso), seeds
//! `HOLON_SOAK_SEED_BLOCKS` blocks, then performs a handful of single-field
//! edits and lets the `[LoroProjection] applied N op(s) in Xms (snapshot Yms
//! ...)` log line surface the per-edit projection cost at scale.
//!
//! Run:
//!   HOLON_SOAK_SEED_BLOCKS=2000 cargo run --release \
//!     --example crdt_incr_bench 2>&1 | tee bench.log
//! then grep:  grep 'BENCH_EDIT\|LoroProjection] applied' bench.log

use std::sync::Arc;
use std::time::Duration;

use holon_api::{QueryLanguage, Value};
use holon_integration_tests::TestEnvironmentBuilder;
use std::collections::HashMap;

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
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,holon_loro=info,holon_latency=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let seed = env_usize("HOLON_SOAK_SEED_BLOCKS", 500);
    let edits = env_usize("HOLON_BENCH_EDITS", 3);

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );
    let rt2 = rt.clone();
    rt.block_on(async move {
        eprintln!("[bench] seeding {seed} blocks…");
        let org = build_org(seed);
        let env = TestEnvironmentBuilder::new()
            .with_org_file("soak.org", org)
            .build(rt2.clone())
            .await?;

        let last = format!("soak-{}", seed - 1);
        let ok = env.wait_for_block(&last, Duration::from_secs(300)).await;
        anyhow::ensure!(ok, "last block {last} never projected");
        env.wait_for_loro_quiescence(Duration::from_secs(60)).await;
        eprintln!("[bench] ingest complete for {seed} blocks; steady-state edits begin");

        // Resolve real block ids for a few middle blocks.
        for e in 0..edits {
            let k = (seed / 2) + e;
            let rows = env
                .query_sql(&format!(
                    "SELECT id FROM block_raw WHERE id LIKE '%soak-{k}' LIMIT 1"
                ))
                .await?;
            let id = rows
                .first()
                .and_then(|r| r.get("id"))
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("no id for soak-{k}"))?;

            // Quiesce, mark, then one single-field edit.
            env.wait_for_loro_quiescence(Duration::from_secs(30)).await;
            eprintln!("BENCH_EDIT n={seed} edit={e} id={id}");
            let mut params = HashMap::new();
            params.insert("id".to_string(), Value::String(id.clone()));
            params.insert("field".to_string(), Value::String("content".to_string()));
            params.insert(
                "value".to_string(),
                Value::String(format!("Edited headline for {id} rev {e}")),
            );
            env.execute_operation("block", "set_field", params).await?;
            env.wait_for_loro_quiescence(Duration::from_secs(30)).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        eprintln!("[bench] done");
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}
