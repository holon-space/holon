//! One block insert, maintained through the production chain `block_raw` ->
//! the per-junction `*_agg` views -> `block` -> `block_with_path`, costs about
//! the same in a vault eight times larger.
//!
//! The oracle is the engine's VM step count, maintenance of every view
//! included. It is deterministic, so the property holds under any load and can
//! range over tree shapes; a maintenance plan that scans an input instead of
//! seeking it shows up as steps that grow with the vault, at any vault size.
//! So the property runs at [`STEP_SIZES`], small enough for the default test
//! timeout, and the wall-clock instrument at the bottom makes the same claim in
//! milliseconds at [`MS_SIZES`], for a release build on a quiet host.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockHierarchySchemaModule;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use proptest::prelude::*;
use tokio::sync::broadcast;
use turso_core::ConnectOpts;
use turso_core::Connection;
use turso_core::Database;

pub(crate) const STEP_SIZES: [usize; 2] = [125, 1000];
const MS_SIZES: [usize; 2] = [1000, 8000];
/// Growth between the two sizes below this factor is flat; an input scan grows
/// by about their ratio, 8.
pub(crate) const FLAT: f64 = 2.0;
const ROOT_PARENT: &str = "sentinel:no_parent";
const PROBES: usize = 5;

/// A block tree: block `i > 0` hangs under block `(i - 1) / fanout`, unless
/// that would make it deeper than `max_depth`; then it starts a new root.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shape {
    fanout: usize,
    max_depth: usize,
}

impl Shape {
    /// Chains are capped at 256 levels: `block_with_path` stores each block's
    /// whole ancestor path, so one unbroken chain makes the view itself
    /// quadratic in size, a cost of its content and not of its upkeep.
    const CHAIN: Shape = Shape {
        fanout: 1,
        max_depth: 256,
    };
    const WIDE: Shape = Shape {
        fanout: usize::MAX,
        max_depth: usize::MAX,
    };
    pub(crate) const FANOUT_8: Shape = Shape {
        fanout: 8,
        max_depth: usize::MAX,
    };

    /// The parent index of each of `n` blocks, `None` for a root.
    pub(crate) fn parents(self, n: usize) -> Vec<Option<usize>> {
        let mut depth = vec![0usize; n];
        (0..n)
            .map(|i| {
                if i == 0 {
                    return None;
                }
                let parent = (i - 1) / self.fanout;
                if depth[parent] + 1 > self.max_depth {
                    return None;
                }
                depth[i] = depth[parent] + 1;
                Some(parent)
            })
            .collect()
    }
}

fn shapes() -> impl Strategy<Value = Shape> {
    let capped_depth = 8usize..=256;
    prop_oneof![
        capped_depth.clone().prop_map(|max_depth| Shape {
            fanout: 1,
            max_depth
        }),
        (
            prop_oneof![2usize..=16, Just(usize::MAX)],
            prop_oneof![Just(usize::MAX), capped_depth]
        )
            .prop_map(|(fanout, max_depth)| Shape { fanout, max_depth }),
    ]
}

pub(crate) fn block_id(i: usize) -> String {
    format!("block:b{i}")
}

pub(crate) fn exec(conn: &Arc<Connection>, sql: &str) {
    conn.execute(sql)
        .unwrap_or_else(|e| panic!("{}: {e}", &sql[..sql.len().min(120)]));
}

/// Writes blocks `range` of the tree `parents`, each with one row in every
/// junction, so every `*_agg` view grows with the vault.
pub(crate) fn seed(conn: &Arc<Connection>, parents: &[Option<usize>], range: Range<usize>) {
    let rows: Vec<usize> = range.collect();
    for chunk in rows.chunks(250) {
        let values = |row: &dyn Fn(usize) -> String| {
            chunk.iter().map(|&i| row(i)).collect::<Vec<_>>().join(", ")
        };
        let blocks = values(&|i| {
            let parent = parents[i].map_or(ROOT_PARENT.to_string(), block_id);
            format!("('{}', '{parent}', 'row {i}')", block_id(i))
        });
        exec(
            conn,
            &format!("INSERT INTO block_raw (id, parent_id, content) VALUES {blocks}"),
        );
        for (table, columns, target) in [
            (
                "block_tags",
                "block_id, tag",
                &(|i: usize| format!("tag{}", i % 17)) as &dyn Fn(usize) -> String,
            ),
            ("block_requires", "block_id, required_id", &|i: usize| {
                block_id(i / 2)
            }),
            ("advice_suppressed", "anchor_id, lesson_id", &|i: usize| {
                format!("lesson:{}", i % 5)
            }),
            (
                "block_contributes_to",
                "block_id, target_id",
                &|i: usize| block_id(i / 3),
            ),
        ] {
            let rows = values(&|i| format!("('{}', '{}')", block_id(i), target(i)));
            exec(
                conn,
                &format!("INSERT INTO {table} ({columns}) VALUES {rows}"),
            );
        }
    }
}

/// VM steps of `sql`, maintenance of every view included.
fn steps_of(conn: &Arc<Connection>, sql: &str) -> u64 {
    let steps = Arc::new(AtomicU64::new(0));
    let counter = steps.clone();
    conn.set_progress_handler(
        1,
        Some(Box::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            false
        })),
    );
    let result = conn.execute(sql);
    conn.set_progress_handler(0, None);
    result.unwrap_or_else(|e| panic!("{sql}: {e}"));
    steps.load(Ordering::Relaxed)
}

fn median<T: Copy + PartialOrd>(mut values: Vec<T>) -> T {
    values.sort_by(|a, b| a.partial_cmp(b).expect("comparable"));
    values[values.len() / 2]
}

/// The two places a new block lands: under the deepest block, and under the
/// first root.
pub(crate) fn probe_parents(parents: &[Option<usize>]) -> [(&'static str, usize); 2] {
    let mut depth = vec![0usize; parents.len()];
    for (i, parent) in parents.iter().enumerate() {
        if let Some(p) = parent {
            depth[i] = depth[*p] + 1;
        }
    }
    let deepest = (0..parents.len())
        .max_by_key(|&i| (depth[i], std::cmp::Reverse(i)))
        .expect("a non-empty tree");
    [("under the deepest block", deepest), ("under a root", 0)]
}

fn insert_leaf(parent: usize, probe: &str) -> String {
    format!(
        "INSERT INTO block_raw (id, parent_id, content) VALUES ('block:probe-{probe}', '{}', \
         'probe')",
        block_id(parent)
    )
}

/// Median VM steps of a leaf insert at each of `places`.
pub(crate) fn upkeep_steps(
    conn: &Arc<Connection>,
    places: &[(&'static str, usize); 2],
    size: usize,
) -> [u64; 2] {
    places.map(|(name, parent)| {
        median(
            (0..PROBES)
                .map(|k| {
                    let probe = format!("{size}-{}-{k}", name.replace(' ', "-"));
                    steps_of(conn, &insert_leaf(parent, &probe))
                })
                .collect(),
        )
    })
}

/// The flat-upkeep property for one pair of measurements.
fn assert_flat(
    what: &str,
    places: &[(&'static str, usize); 2],
    small: [u64; 2],
    large: [u64; 2],
) -> Result<(), TestCaseError> {
    let [s_size, l_size] = STEP_SIZES;
    for ((name, _), (s, l)) in places.iter().zip(small.into_iter().zip(large)) {
        let ratio = l as f64 / s as f64;
        prop_assert!(
            ratio < FLAT,
            "{what}: one block insert {name} takes {s} VM steps at {s_size} blocks and {l} at \
             {l_size} ({ratio:.2}x, flat is below {FLAT}x): its maintenance scans an input that \
             grows with the vault"
        );
    }
    Ok(())
}

pub(crate) struct Vault {
    _dir: tempfile::TempDir,
    backend: TursoBackend,
    handle: DbHandle,
    pub(crate) conn: Arc<Connection>,
    _db: Arc<Database>,
}

impl Vault {
    pub(crate) async fn close(self) {
        self.handle.shutdown().await.expect("shutdown");
        drop(self.backend);
    }
}

pub(crate) fn open(path: &Path) -> (Arc<Database>, TursoBackend, DbHandle, Arc<Connection>) {
    let db = TursoBackend::open_database(path).expect("open db file");
    let (backend, handle) =
        TursoBackend::new(db.clone(), broadcast::channel(64).0).expect("backend");
    let conn = db.connect(ConnectOpts::new()).expect("connect");
    (db, backend, handle, conn)
}

/// The tables first and the views over the seeded rows after, as a vault
/// that grew under an older build and is then booted.
async fn vault(parents: &[Option<usize>]) -> Vault {
    let dir = tempfile::tempdir().expect("tempdir");
    let (db, backend, handle, conn) = open(&dir.path().join("vault.db"));
    CoreSchemaModule.ensure_schema(&handle).await.expect("core");
    BlockSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("junctions");
    seed(&conn, parents, 0..parents.len());
    BlockMatviewSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block matview chain");
    BlockHierarchySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block_with_path");
    Vault {
        _dir: dir,
        backend,
        handle,
        conn,
        _db: db,
    }
}

async fn upkeep_at(shape: Shape, size: usize) -> [u64; 2] {
    let parents = shape.parents(size);
    let vault = vault(&parents).await;
    let steps = upkeep_steps(&vault.conn, &probe_parents(&parents), size);
    vault.close().await;
    steps
}

fn check(rt: &tokio::runtime::Runtime, shape: Shape) -> Result<(), TestCaseError> {
    let [s_size, l_size] = STEP_SIZES;
    let start = Instant::now();
    let (small, large) = rt.block_on(async {
        (
            upkeep_at(shape, s_size).await,
            upkeep_at(shape, l_size).await,
        )
    });
    eprintln!(
        "[upkeep] {shape:?}: {s_size} blocks {small:?} steps, {l_size} blocks {large:?} steps, {:.1}s",
        start.elapsed().as_secs_f64()
    );
    assert_flat(
        &format!("{shape:?}"),
        &probe_parents(&shape.parents(s_size)),
        small,
        large,
    )
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

#[test]
fn one_block_insert_costs_as_many_steps_in_a_vault_eight_times_larger() {
    let rt = runtime();
    for shape in [Shape::FANOUT_8, Shape::WIDE, Shape::CHAIN] {
        check(&rt, shape).unwrap_or_else(|e| panic!("{e}"));
    }
    proptest!(ProptestConfig::with_cases(4), |(shape in shapes())| {
        check(&rt, shape)?;
    });
}

/// Median milliseconds per leaf insert at each place, over `runs` rounds.
fn upkeep_ms(conn: &Arc<Connection>, places: &[(&'static str, usize); 2], runs: usize) -> [f64; 2] {
    places.map(|(name, parent)| {
        median(
            (0..runs)
                .map(|k| {
                    let sql = insert_leaf(parent, &format!("ms-{}-{k}", name.replace(' ', "-")));
                    let start = Instant::now();
                    exec(conn, &sql);
                    start.elapsed().as_secs_f64() * 1e3
                })
                .collect(),
        )
    })
}

#[test]
#[ignore = "timing instrument; run explicitly on a release build on a quiet host"]
fn one_block_insert_takes_as_long_at_8k_blocks_as_at_1k() {
    const RUNS: usize = 9;
    let rt = runtime();
    let shape = Shape::FANOUT_8;
    let [small, large] = MS_SIZES.map(|size| {
        rt.block_on(async {
            let parents = shape.parents(size);
            let vault = vault(&parents).await;
            let ms = upkeep_ms(&vault.conn, &probe_parents(&parents), RUNS);
            vault.close().await;
            ms
        })
    });
    for (i, name) in ["under the deepest block", "under a root"]
        .iter()
        .enumerate()
    {
        let ratio = large[i] / small[i];
        let [s_size, l_size] = MS_SIZES;
        eprintln!(
            "[upkeep-ms] {shape:?} {name}: median of {RUNS} {:.3} ms at {s_size} blocks, {:.3} ms \
             at {l_size} ({ratio:.2}x)",
            small[i], large[i]
        );
        assert!(
            ratio < FLAT,
            "{name}: {ratio:.2}x from {s_size} to {l_size} blocks, flat is below {FLAT}x"
        );
    }
}
