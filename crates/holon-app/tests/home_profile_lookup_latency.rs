//! What the DERIVED home-profile lookup costs in milliseconds, on a real
//! file-backed Turso database.
//!
//! The read-COUNT law is fixed elsewhere
//! (`holon-orgmode/tests/home_authority_ancestor_walk_cost.rs`: the walk costs
//! `depth + 1` authoritative point reads, deterministically). That harness
//! reads from a `BTreeMap`, so it can say nothing about wall-clock. This one
//! converts the law into time against the 200 ms p95 interaction budget, which
//! requires real storage — hence a file on disk, asserted to be one.
//!
//! It does NOT decide whether the home-profile column should exist. It
//! produces the number that decision needs.
//!
//! @pbt kind harness
//! @pbt covers home-profile-lookup-latency — the derived lookup's wall-clock at
//! keystone scale and 10x it, cold and warm, against the p95 budget
//! @pbt overlaps home_authority_ancestor_walk_cost — that fixes the read COUNT
//! and the answer; this one prices the reads

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::core::queryable_cache::QueryableCache;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::block::Block;
use holon_api::live_data::home_by::HomeAuthority;
use holon_filesystem::BlockReader;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::HomeBurstMemo;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// Blocks in the seeded database. The keystone's own state sits at 20–34
/// blocks (its `inv-sql-budget` telemetry prints `state=b20`…`b34`), so 32 is
/// its scale; 320 is the 10x point that shows whether the law holds or the
/// per-read cost moves with table size.
const SCALES: &[usize] = &[32, 320];

/// Depths measured end-to-end. 1–2 is the keystone's real tree; 4 is its
/// plausible ceiling.
const DEPTHS: &[usize] = &[1, 2, 4];

const RUNS_PER_ARM: usize = 200;

/// The p95 interaction→projection-visible budget the whole lookup must fit
/// inside, with room to spare — a home-profile resolve is one step of an
/// interaction, never its whole cost.
const SLO_P95: Duration = Duration::from_millis(200);

struct Samples(Vec<Duration>);

impl Samples {
    /// min / p50 / p95 / max. Percentiles by nearest-rank on the sorted
    /// samples — the DISTRIBUTION, never a single number standing in for it.
    fn stats(&self) -> (Duration, Duration, Duration, Duration) {
        let mut v = self.0.clone();
        v.sort();
        let at = |q: f64| {
            let rank = ((q * v.len() as f64).ceil() as usize).max(1) - 1;
            v[rank.min(v.len() - 1)]
        };
        (v[0], at(0.50), at(0.95), v[v.len() - 1])
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Seed `total` blocks: chains of each measured depth under their own page,
/// padded with filler leaves so the table reaches the target size.
async fn seed(handle: &holon::storage::turso::DbHandle, total: usize) -> Vec<(usize, String)> {
    let mut targets = Vec::new();
    let mut written = 0usize;

    for &depth in DEPTHS {
        let page = format!("block:page-d{depth}");
        insert(handle, &page, "sentinel:no_parent").await;
        tag_page(handle, &page).await;
        written += 1;

        let mut parent = page;
        for step in 0..depth {
            let id = format!("block:d{depth}-s{step}");
            insert(handle, &id, &parent).await;
            written += 1;
            parent = id;
        }
        targets.push((depth, parent));
    }

    // Filler under a page of its own, so the measured chains keep their depth.
    let filler_page = "block:filler-page".to_string();
    insert(handle, &filler_page, "sentinel:no_parent").await;
    tag_page(handle, &filler_page).await;
    written += 1;
    while written < total {
        insert(handle, &format!("block:filler-{written}"), &filler_page).await;
        written += 1;
    }
    targets
}

async fn insert(handle: &holon::storage::turso::DbHandle, id: &str, parent: &str) {
    handle
        .execute(
            // ALLOW(sole_block_writer): seeding a fixed topology for a
            // measurement; no operation builds an arbitrary-depth chain.
            &format!(
                "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                 created_at, updated_at) VALUES ('{id}', '{parent}', 'a0', '{id}', 'text', 0, 0)"
            ),
            vec![],
        )
        .await
        .unwrap_or_else(|e| panic!("seed {id}: {e}"));
}

async fn tag_page(handle: &holon::storage::turso::DbHandle, id: &str) {
    handle
        .execute(
            &format!(
                "INSERT INTO block_tags (block_id, tag) VALUES ('{id}', '{}')",
                holon_api::PAGE_TAG
            ),
            vec![],
        )
        .await
        .expect("tag page");
}

/// The measurement. One report block, printed verbatim, and the assertions
/// that make it trustworthy.
/// `#[ignore]` BY DESIGN, the same reason the LogSeq oracle tests carry it: a
/// plain `cargo test` must never report this as passing when it measured
/// nothing. It shows as `ignored` until asked for, and it only measures in
/// RELEASE:
///
/// ```text
/// cargo test --release -p holon-app --test home_profile_lookup_latency -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "release-only wall-clock measurement; run with --ignored"]
async fn the_derived_home_profile_lookup_fits_the_interaction_budget() {
    // Wall-clock is RELEASE-only here, as it is for the soak measurements: a
    // debug build's timings are several times production's, so publishing them
    // against the interaction budget would be a number that looks like evidence
    // and is not. Skip loudly rather than write one.
    assert!(
        !cfg!(debug_assertions),
        "[home-profile-latency] a debug build prices nothing — run this in RELEASE: \
         `cargo test --release -p holon-app --test home_profile_lookup_latency -- --ignored`"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    // The artifact stamps its own provenance: a timing file that does not say
    // which build produced it, on what machine load, is unreadable evidence.
    let mut report = format!(
        "derived home-profile lookup — file-backed Turso, per-lookup wall-clock\n\
         profile: RELEASE · {RUNS_PER_ARM} runs per arm, arms alternated per iteration\n\
         percentiles by nearest rank · budget under test: {} ms p95\n\
         NOTE: single-digit-ms operations need a QUIET machine; a run sharing the\n\
         build slots with other lanes measures scheduler noise, not the lookup.\n",
        SLO_P95.as_millis()
    );

    for &scale in SCALES {
        let db_path = dir.path().join(format!("bench-{scale}.db"));
        let db = TursoBackend::open_database(&db_path).expect("open file-backed db");
        let (cdc_tx, _rx) = tokio::sync::broadcast::channel(1024);
        let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("backend");
        handle
            .execute_ddl("PRAGMA foreign_keys = ON")
            .await
            .expect("FK pragma");
        for module in ["core", "block", "matview"] {
            match module {
                "core" => holon_turso::schema_modules::CoreSchemaModule
                    .ensure_schema(&handle)
                    .await
                    .expect("core schema"),
                "block" => BlockSchemaModule
                    .ensure_schema(&handle)
                    .await
                    .expect("block schema"),
                _ => BlockMatviewSchemaModule
                    .ensure_schema(&handle)
                    .await
                    .expect("matview schema"),
            }
        }

        let targets = seed(&handle, scale).await;

        // DISK CHECKED: the point of this harness is that storage is real.
        assert!(
            db_path.exists(),
            "no database file at {} — an in-memory run would price nothing",
            db_path.display()
        );
        let bytes = std::fs::metadata(&db_path).expect("stat db").len();
        assert!(bytes > 0, "database file is empty; nothing was written");
        assert_eq!(
            block_rows(&handle).await,
            scale,
            "the seeded row count must be the scale under test"
        );

        let sql = Arc::new(holon::core::SqlOperationProvider::with_edge_fields(
            handle.clone(),
            "block_raw".to_string(),
            "block".to_string(),
            "block".to_string(),
            BlockSchemaModule.edge_fields(),
        ));
        let mut type_def = Block::type_definition();
        type_def.name = "block_raw".to_string();
        let cache = Arc::new(
            QueryableCache::<Block>::new(handle.clone(), type_def)
                .await
                .expect("cache"),
        );
        let blocks = Arc::new(holon::core::sql_block_operations::SqlBlockOperations::new(
            sql,
            cache.clone(),
        ));
        let reader: Arc<dyn BlockReader> =
            Arc::new(holon_app::turso_seams::CacheBlockReader::new(cache));
        let authority = BlockHomeAuthority::new(reader.clone(), blocks);

        report.push_str(&format!(
            "\n  scale {scale} blocks ({bytes} bytes on disk)\n"
        ));

        for &(depth, ref target) in &targets {
            let uri = holon_api::EntityUri::parse(target).expect("target uri");

            // COLD: the first lookup of this chain, before anything warms it.
            let cold_start = Instant::now();
            authority
                .locate(target, &mut HomeBurstMemo::default())
                .await
                .expect("locate")
                .expect("target present");
            let cold = cold_start.elapsed();

            let mut walk = Vec::with_capacity(RUNS_PER_ARM);
            let mut point = Vec::with_capacity(RUNS_PER_ARM);
            for _ in 0..RUNS_PER_ARM {
                // A: the whole derived lookup.
                let t = Instant::now();
                authority
                    .locate(target, &mut HomeBurstMemo::default())
                    .await
                    .expect("locate")
                    .expect("target present");
                walk.push(t.elapsed());

                // B: ONE point read — what a stored column would cost.
                let t = Instant::now();
                reader
                    .get_block_authoritative(&uri)
                    .await
                    .expect("point read")
                    .expect("target present");
                point.push(t.elapsed());
            }

            assert_eq!(
                (walk.len(), point.len()),
                (RUNS_PER_ARM, RUNS_PER_ARM),
                "both arms must have run {RUNS_PER_ARM} times at depth {depth}, scale {scale}"
            );
            assert!(
                walk.iter().all(|d| *d > Duration::ZERO),
                "a lookup that took zero time measured nothing"
            );

            let (w_min, w_p50, w_p95, w_max) = Samples(walk).stats();
            let (p_min, p_p50, p_p95, p_max) = Samples(point).stats();
            report.push_str(&format!(
                "    depth {depth}: derived warm min {:.3} p50 {:.3} p95 {:.3} max {:.3} ms · \
                 COLD first {:.3} ms\n              1 point read  min {:.3} p50 {:.3} p95 {:.3} \
                 max {:.3} ms\n",
                ms(w_min),
                ms(w_p50),
                ms(w_p95),
                ms(w_max),
                ms(cold),
                ms(p_min),
                ms(p_p50),
                ms(p_p95),
                ms(p_max),
            ));

            assert!(
                w_p95 < SLO_P95,
                "the derived lookup's p95 ({:.3} ms) reached the {} ms interaction budget at \
                 depth {depth}, scale {scale} — that number IS the argument for a stored column",
                ms(w_p95),
                SLO_P95.as_millis(),
            );
        }
    }

    println!("{report}");
    // The artifact is a TRACKED file and part of a freeze, so a re-run must not
    // rewrite it by default — an independent reproduction would otherwise break
    // the SET-HASH it is trying to confirm. Asserting and printing always
    // happen; publishing is asked for explicitly.
    if std::env::var_os("HOLON_WRITE_LATENCY_DOC").is_some() {
        let out = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/Testing/HomeProfileLookupLatency.txt");
        std::fs::write(&out, &report).expect("write the measurement");
        println!("[home-profile-latency] wrote {}", out.display());
    } else {
        println!(
            "[home-profile-latency] not written to docs/Testing/HomeProfileLookupLatency.txt \
             (set HOLON_WRITE_LATENCY_DOC=1 to publish)"
        );
    }
}

async fn block_rows(handle: &holon::storage::turso::DbHandle) -> usize {
    let rows = handle
        .query(
            "SELECT COUNT(*) AS n FROM block_raw WHERE id != 'sentinel:no_parent'",
            HashMap::new(),
        )
        .await
        .expect("count blocks");
    rows.into_iter()
        .next()
        .map(|r| match r.get("n") {
            Some(holon_api::Value::Integer(i)) => *i,
            other => panic!("COUNT(*) did not come back as an integer: {other:?}"),
        })
        .expect("a count row") as usize
}
