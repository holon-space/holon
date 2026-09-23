#![cfg(feature = "pbt")]
//! What an `owning_page` read costs each write authority on a deep, large
//! vault: the walk from the deepest block of every chain to its document.
//!
//! The figures are REPORTED, not asserted — a wall-clock bound here would
//! measure the build profile and the machine. What is asserted is that every
//! walk names the chain's own document.
//!
//! Shape (env-overridable): `HOLON_OWNING_PAGE_DOCS` documents (default 10),
//! each holding `HOLON_OWNING_PAGE_CHAINS` chains (default 50) of
//! `HOLON_OWNING_PAGE_DEPTH` nested headlines (default 20) — 10 000 blocks.
//!
//! @pbt kind harness
//! @pbt covers write-authority-owning-page-latency — the cost of the
//!   owning-page walk on each write authority at vault scale

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityUri;
use holon_core::OwningPage;
use holon_core::WriteAuthorityReads;
use holon_integration_tests::TestEnvironmentBuilder;

const ROUNDS: usize = 3;

fn env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Ok(raw) => raw
            .parse()
            .unwrap_or_else(|e| panic!("{key}='{raw}' is not a count: {e}")),
        Err(std::env::VarError::NotPresent) => default,
        Err(e) => panic!("{key} unreadable: {e}"),
    }
}

struct Shape {
    docs: usize,
    chains: usize,
    depth: usize,
}

impl Shape {
    fn from_env() -> Self {
        Self {
            docs: env_usize("HOLON_OWNING_PAGE_DOCS", 10),
            chains: env_usize("HOLON_OWNING_PAGE_CHAINS", 50),
            depth: env_usize("HOLON_OWNING_PAGE_DEPTH", 20),
        }
    }

    fn document(&self, doc: usize) -> String {
        let mut out = format!("#+TITLE: Owning {doc}\n#+ID: op-doc-{doc:03}\n");
        for chain in 0..self.chains {
            for level in 1..=self.depth {
                out.push_str(&"*".repeat(level));
                out.push_str(&format!(
                    " N {doc}-{chain}-{level}\n:PROPERTIES:\n:ID: op-{doc:03}-{chain:03}-{level:02}\n:END:\n"
                ));
            }
        }
        out
    }

    /// `(deepest block, its document)` for every chain.
    fn probes(&self) -> Vec<(EntityUri, EntityUri)> {
        (0..self.docs)
            .flat_map(|doc| {
                (0..self.chains).map(move |chain| {
                    (
                        EntityUri::block(&format!("op-{doc:03}-{chain:03}-{:02}", self.depth)),
                        EntityUri::block(&format!("op-doc-{doc:03}")),
                    )
                })
            })
            .collect()
    }
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

async fn measure<F, Fut>(label: &str, probes: &[(EntityUri, EntityUri)], walk: F) -> String
where
    F: Fn(EntityUri) -> Fut,
    Fut: std::future::Future<Output = holon_core::Result<OwningPage>>,
{
    let mut samples = Vec::with_capacity(probes.len() * ROUNDS);
    for _ in 0..ROUNDS {
        for (leaf, doc) in probes {
            let started = Instant::now();
            let answer = walk(leaf.clone())
                .await
                .unwrap_or_else(|e| panic!("{label}: owning_page({leaf}): {e}"));
            samples.push(started.elapsed());
            match answer {
                OwningPage::Page(page) => assert_eq!(
                    &page.block.id, doc,
                    "{label}: {leaf} walked to the wrong document"
                ),
                other => panic!("{label}: {leaf} has no owning page: {other:?}"),
            }
        }
    }
    let first = samples[0];
    samples.sort();
    format!(
        "OWNING_PAGE_LATENCY {label}: n={} first={first:?} p50={:?} p95={:?} max={:?}",
        samples.len(),
        percentile(&samples, 0.50),
        percentile(&samples, 0.95),
        samples[samples.len() - 1]
    )
}

#[test]
fn owning_page_cost_on_a_deep_vault() {
    let shape = Shape::from_env();
    let probes = shape.probes();
    let files: Vec<(String, String)> = (0..shape.docs)
        .map(|d| (format!("Owning/Doc{d:03}.org"), shape.document(d)))
        .collect();
    println!(
        "OWNING_PAGE_SHAPE docs={} chains={} depth={} blocks={} profile={}",
        shape.docs,
        shape.chains,
        shape.depth,
        shape.docs * shape.chains * shape.depth,
        if cfg!(debug_assertions) {
            "debug-assertions (test/dev)"
        } else {
            "optimized (release)"
        }
    );

    let rt = runtime();
    rt.clone().block_on(async move {
        let mut report = Vec::new();

        let mut builder = TestEnvironmentBuilder::new();
        for (name, body) in &files {
            builder = builder.with_org_file(name.clone(), body.clone());
        }
        let booting = Instant::now();
        let loro_env = builder
            .build(rt.clone())
            .await
            .expect("boot the Loro vault");
        loro_env
            .wait_for_loro_quiescence(Duration::from_secs(600))
            .await;
        println!("OWNING_PAGE_BOOT loro {:?}", booting.elapsed());
        let loro = loro_env
            .injector()
            .expect("booted injector")
            .resolve::<dyn WriteAuthorityReads>();
        report.push(
            measure("loro one-read", &probes, |leaf| {
                let loro = loro.clone();
                async move { loro.owning_page(&leaf).await }
            })
            .await,
        );
        report.push(
            measure("loro per-hop", &probes, |leaf| {
                let loro = loro.clone();
                async move { holon_core::owning_page_by_hops(loro.as_ref(), &leaf).await }
            })
            .await,
        );
        drop(loro_env);

        let mut builder = TestEnvironmentBuilder::new().without_loro();
        for (name, body) in &files {
            builder = builder.with_org_file(name.clone(), body.clone());
        }
        let booting = Instant::now();
        let sql_env = builder.build(rt).await.expect("boot the SqlOnly vault");
        println!("OWNING_PAGE_BOOT sql {:?}", booting.elapsed());
        let sql = Arc::new(holon::core::sql_write_authority::SqlWriteAuthority::new(
            sql_env.engine().db_handle().clone(),
        ));
        report.push(
            measure("sql per-hop", &probes, |leaf| {
                let sql = sql.clone();
                async move { sql.owning_page(&leaf).await }
            })
            .await,
        );

        for line in report {
            println!("{line}");
        }
    });
}
