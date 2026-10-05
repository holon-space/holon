//! Spike S0 (throwaway): hosts for one shared entity index over N declaring
//! types, measured through the pinned fork and the real `DbHandle`.
//!
//! One process measures one configuration, set by env:
//! `S0_N` entities, `S0_TYPES` declaring types (block + generated ones),
//! `S0_OPTION` = `A` (one UNION ALL matview + indexes + chained backlinks
//! matview) or `B` (one matview per type + one-shot UNION ALL queries).
//! It prints one `S0RESULT {json}` line.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use turso::Value as V;

const ROOT_PARENT: &str = "sentinel:no_parent";
const HOT: &str = "block:hot";
const HOT_BACKLINKS: usize = 1000;
const WORDS: [&str; 24] = [
    "onion", "garlic", "simmer", "plan", "review", "meeting", "budget", "tomato", "basil",
    "release", "spike", "index", "search", "garden", "letter", "invoice", "travel", "kitchen",
    "oven", "salt", "pepper", "draft", "notes", "agenda",
];

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn text(&mut self, i: usize, words: usize) -> String {
        let mut s: Vec<String> = (0..words)
            .map(|_| WORDS[self.below(WORDS.len())].to_string())
            .collect();
        s.push(format!("tok{i}"));
        s.join(" ")
    }
}

fn env_usize(name: &str) -> usize {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set"))
        .parse()
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn rss_kib() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .expect("rss")
}

fn pct(mut v: Vec<f64>) -> serde_json::Value {
    assert!(!v.is_empty());
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |p: f64| v[((v.len() as f64 - 1.0) * p).round() as usize];
    serde_json::json!({"n": v.len(), "p50": at(0.5), "p95": at(0.95), "max": v[v.len()-1]})
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Type 0 is block (block_raw); types 1.. are generated tables `t{k}`.
fn type_names(types: usize) -> Vec<String> {
    (0..types)
        .map(|k| {
            if k == 0 {
                "block".into()
            } else {
                format!("t{k}")
            }
        })
        .collect()
}

/// The entity count of type `k` and the uri of its `j`-th entity.
fn uri(k: usize, j: usize) -> String {
    if k == 0 {
        format!("block:b{j}")
    } else {
        format!("t{k}:e{j}")
    }
}

async fn q(h: &DbHandle, sql: &str, params: Vec<V>) -> usize {
    h.query_positional(sql, params)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .len()
}

async fn ddl(h: &DbHandle, sql: &str) -> f64 {
    let t = Instant::now();
    h.execute_ddl(sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    ms(t.elapsed())
}

async fn page_bytes(h: &DbHandle) -> i64 {
    let rows = h
        .query_positional("PRAGMA page_count", vec![])
        .await
        .expect("page_count");
    let n = match rows[0].values().next() {
        Some(holon_api::Value::Integer(n)) => *n,
        other => panic!("page_count {other:?}"),
    };
    n * 4096
}

async fn seed(h: &DbHandle, n: usize, types: usize, rng: &mut Lcg) -> Vec<usize> {
    let per_typed = if types > 1 { n / 2 / (types - 1) } else { 0 };
    let blocks = n - per_typed * (types - 1);
    let mut counts = vec![blocks];
    let mut stmts = vec![(
        "INSERT INTO block_raw (id, parent_id, content) VALUES (?, ?, ?)".to_string(),
        vec![
            V::Text(HOT.into()),
            V::Text(ROOT_PARENT.into()),
            V::Text("hot target".into()),
        ],
    )];
    for j in 0..blocks {
        let parent = if j == 0 {
            ROOT_PARENT.to_string()
        } else {
            uri(0, j / 8)
        };
        stmts.push((
            "INSERT INTO block_raw (id, parent_id, content) VALUES (?, ?, ?)".into(),
            vec![V::Text(uri(0, j)), V::Text(parent), V::Text(rng.text(j, 6))],
        ));
        if stmts.len() >= 2000 {
            h.transaction(std::mem::take(&mut stmts))
                .await
                .expect("seed blocks");
        }
    }
    for k in 1..types {
        h.execute_ddl(&format!(
            "CREATE TABLE t{k} (id TEXT PRIMARY KEY, parent_id TEXT, position INTEGER, title TEXT, body TEXT)"
        ))
        .await
        .expect("typed table");
        for j in 0..per_typed {
            stmts.push((
                format!(
                    "INSERT INTO t{k} (id, parent_id, position, title, body) VALUES (?, ?, ?, ?, ?)"
                ),
                vec![
                    V::Text(uri(k, j)),
                    V::Text(format!("t{k}:parent{}", j / 20)),
                    V::Integer(j as i64),
                    V::Text(rng.text(j, 3)),
                    V::Text(rng.text(j, 8)),
                ],
            ));
            if stmts.len() >= 2000 {
                h.transaction(std::mem::take(&mut stmts))
                    .await
                    .expect("seed typed");
            }
        }
        counts.push(per_typed);
    }
    h.execute_ddl(
        "CREATE TABLE entity_links (id INTEGER PRIMARY KEY, source_uri TEXT, target TEXT, kind TEXT, resolved_id TEXT)",
    )
    .await
    .expect("links");
    h.execute_ddl("CREATE INDEX idx_entity_links_resolved ON entity_links(resolved_id)")
        .await
        .expect("links idx");
    let link = "INSERT INTO entity_links (source_uri, target, kind, resolved_id) VALUES (?, ?, 'entity', ?)";
    // Background links: one per two entities, random source and target.
    for _ in 0..n / 2 {
        let (sk, tk) = (rng.below(types), rng.below(types));
        let (s, t) = (
            uri(sk, rng.below(counts[sk])),
            uri(tk, rng.below(counts[tk])),
        );
        stmts.push((
            link.into(),
            vec![V::Text(s), V::Text(t.clone()), V::Text(t)],
        ));
        if stmts.len() >= 2000 {
            h.transaction(std::mem::take(&mut stmts))
                .await
                .expect("seed links");
        }
    }
    // 1k backlinks to HOT from distinct sources over all types.
    for i in 0..HOT_BACKLINKS {
        let k = i % types;
        let s = uri(k, (i / types) % counts[k]);
        stmts.push((
            link.into(),
            vec![V::Text(s), V::Text(HOT.into()), V::Text(HOT.into())],
        ));
    }
    h.transaction(stmts).await.expect("seed tail");
    counts
}

/// Branch of type `k`: `(uri, type, title, search_text)`.
fn branch(k: usize, name: &str) -> String {
    if k == 0 {
        format!(
            "SELECT id AS uri, '{name}' AS type, content AS title, content AS search_text FROM block_raw WHERE id != '{ROOT_PARENT}'"
        )
    } else {
        format!(
            "SELECT id AS uri, '{name}' AS type, title, title || ' ' || body AS search_text FROM {name}"
        )
    }
}

/// Single-row keystroke-like writes, each its own transaction (the
/// production write path).
async fn writes(h: &DbHandle, counts: &[usize], rng: &mut Lcg, rounds: usize) -> serde_json::Value {
    let (mut block, mut typed) = (vec![], vec![]);
    for r in 0..rounds {
        let j = rng.below(counts[0]);
        let t = Instant::now();
        h.transaction(vec![(
            "UPDATE block_raw SET content = ? WHERE id = ?".into(),
            vec![
                V::Text(format!("{} edit{r}", rng.text(j, 6))),
                V::Text(uri(0, j)),
            ],
        )])
        .await
        .expect("block write");
        block.push(ms(t.elapsed()));
        if counts.len() > 1 {
            let j = rng.below(counts[1]);
            let t = Instant::now();
            h.transaction(vec![(
                "UPDATE t1 SET title = ? WHERE id = ?".into(),
                vec![
                    V::Text(format!("{} edit{r}", rng.text(j, 3))),
                    V::Text(uri(1, j)),
                ],
            )])
            .await
            .expect("typed write");
            typed.push(ms(t.elapsed()));
        }
    }
    // A block that is a source of a HOT backlink (chains into backlinks).
    let mut source = vec![];
    for r in 0..rounds {
        let j = r % (HOT_BACKLINKS / counts.len());
        let t = Instant::now();
        h.transaction(vec![(
            "UPDATE block_raw SET content = ? WHERE id = ?".into(),
            vec![V::Text(format!("source edit {r}")), V::Text(uri(0, j))],
        )])
        .await
        .expect("source write");
        source.push(ms(t.elapsed()));
    }
    let mut link = vec![];
    for r in 0..rounds {
        let t = Instant::now();
        h.transaction(vec![(
            "INSERT INTO entity_links (source_uri, target, kind, resolved_id) VALUES (?, ?, 'entity', ?)".into(),
            vec![V::Text(uri(0, r)), V::Text(HOT.into()), V::Text(HOT.into())],
        )])
        .await
        .expect("link insert");
        link.push(ms(t.elapsed()));
    }
    serde_json::json!({"block": pct(block), "typed": if typed.is_empty() { serde_json::Value::Null } else { pct(typed) }, "backlink_source": pct(source), "link_insert": pct(link)})
}

async fn timed(
    h: &DbHandle,
    sql: &str,
    args: &[Vec<V>],
    budget: Duration,
) -> (serde_json::Value, usize) {
    let mut v = vec![];
    let mut hits = 0;
    let start = Instant::now();
    for _ in 0..3 {
        for a in args {
            let t = Instant::now();
            hits = q(h, sql, a.clone()).await;
            v.push(ms(t.elapsed()));
            if start.elapsed() > budget {
                return (pct(v), hits);
            }
        }
    }
    (pct(v), hits)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "S0 timing instrument; driven by the scratchpad capabilities/run-s0.sh"]
async fn entity_index_spike() {
    let n = env_usize("S0_N");
    let types = env_usize("S0_TYPES");
    let option = std::env::var("S0_OPTION").expect("S0_OPTION");
    let names = type_names(types);
    let dir = tempfile::tempdir().unwrap();
    let db = TursoBackend::open_database(dir.path().join("s0.db")).expect("db");
    let (cdc_tx, _rx) = tokio::sync::broadcast::channel(1 << 16);
    let (backend, h) = TursoBackend::new(db, cdc_tx).expect("backend");
    std::mem::forget(backend);
    CoreSchemaModule.ensure_schema(&h).await.expect("core");
    BlockSchemaModule.ensure_schema(&h).await.expect("block");
    BlockMatviewSchemaModule
        .ensure_schema(&h)
        .await
        .expect("block matview");

    let mut rng = Lcg(42);
    let t = Instant::now();
    let counts = seed(&h, n, types, &mut rng).await;
    let seed_ms = ms(t.elapsed());
    let base_writes = writes(&h, &counts, &mut rng, 200).await;

    let (rss0, bytes0) = (rss_kib(), page_bytes(&h).await);
    let mut create = serde_json::Map::new();
    let union_of = |cols: &str, pred: &str| -> String {
        names
            .iter()
            .map(|name| format!("SELECT {cols}, '{name}' AS type FROM ei_{name} WHERE {pred}"))
            .collect::<Vec<_>>()
            .join(" UNION ALL ")
    };
    let (search_sql, point_sql, backlinks_sql);
    match option.as_str() {
        "A" => {
            let body = names
                .iter()
                .enumerate()
                .map(|(k, nm)| branch(k, nm))
                .collect::<Vec<_>>()
                .join(" UNION ALL ");
            create.insert(
                "matview".into(),
                ddl(
                    &h,
                    &format!("CREATE MATERIALIZED VIEW entity_index AS {body}"),
                )
                .await
                .into(),
            );
            create.insert(
                "index_uri".into(),
                ddl(&h, "CREATE INDEX idx_entity_index_uri ON entity_index(uri)")
                    .await
                    .into(),
            );
            create.insert(
                "backlinks_matview".into(),
                ddl(
                    &h,
                    "CREATE MATERIALIZED VIEW entity_backlinks AS SELECT l.resolved_id AS target, l.source_uri AS source_uri, e.title AS title, e.type AS type FROM entity_links l JOIN entity_index e ON l.source_uri = e.uri",
                )
                .await
                .into(),
            );
            create.insert(
                "backlinks_index".into(),
                ddl(
                    &h,
                    "CREATE INDEX idx_entity_backlinks_target ON entity_backlinks(target)",
                )
                .await
                .into(),
            );
            search_sql =
                "SELECT uri, type, title FROM entity_index WHERE search_text LIKE ? LIMIT 50"
                    .to_string();
            point_sql = "SELECT type, title FROM entity_index WHERE uri = ?".to_string();
            backlinks_sql =
                "SELECT source_uri, title, type FROM entity_backlinks WHERE target = ?".to_string();
        }
        "B" => {
            for (k, nm) in names.iter().enumerate() {
                let sel = branch(k, nm).replace(&format!(", '{nm}' AS type"), "");
                create.insert(
                    format!("matview_{nm}"),
                    ddl(&h, &format!("CREATE MATERIALIZED VIEW ei_{nm} AS {sel}"))
                        .await
                        .into(),
                );
                create.insert(
                    format!("index_{nm}"),
                    ddl(&h, &format!("CREATE INDEX idx_ei_{nm}_uri ON ei_{nm}(uri)"))
                        .await
                        .into(),
                );
            }
            search_sql = format!(
                "SELECT * FROM ({}) LIMIT 50",
                union_of("uri, title", "search_text LIKE ?1")
            );
            point_sql = union_of("title", "uri = ?1");
            backlinks_sql = format!(
                "SELECT l.source_uri, e.title, e.type FROM entity_links l JOIN ({}) e ON e.uri = l.source_uri WHERE l.resolved_id = ?1",
                union_of("uri, title", "1")
            );
        }
        other => panic!("S0_OPTION {other}"),
    }
    let create_total: f64 = create.values().map(|v| v.as_f64().unwrap()).sum();
    let (rss1, bytes1) = (rss_kib(), page_bytes(&h).await);

    // Correctness: the index holds every entity, and the writes above are visible.
    let total: usize = counts.iter().sum::<usize>() + 1;
    let idx_rows = match option.as_str() {
        "A" => q(&h, "SELECT uri FROM entity_index", vec![]).await,
        _ => {
            let mut s = 0;
            for nm in &names {
                s += q(&h, &format!("SELECT uri FROM ei_{nm}"), vec![]).await;
            }
            s
        }
    };
    assert_eq!(idx_rows, total, "the index must hold every entity");

    let common: Vec<Vec<V>> = WORDS
        .iter()
        .take(10)
        .map(|w| vec![V::Text(format!("%{w}%"))])
        .collect();
    let rare: Vec<Vec<V>> = (0..10)
        .map(|i| vec![V::Text(format!("%tok{}x%", 7 * i + 3))])
        .collect();
    let none: Vec<Vec<V>> = (0..10)
        .map(|i| vec![V::Text(format!("%zzq{i}%"))])
        .collect();
    let budget = Duration::from_secs(30);
    let (s_common, h_common) = timed(&h, &search_sql, &common, budget).await;
    let (s_rare, _) = timed(&h, &search_sql, &rare, budget).await;
    let (s_none, h_none) = timed(&h, &search_sql, &none, budget).await;
    assert_eq!(h_common, 50);
    assert_eq!(h_none, 0);
    let points: Vec<Vec<V>> = (0..20)
        .map(|i| vec![V::Text(uri(i % types, i * 37 % counts[i % types]))])
        .collect();
    let (s_point, h_point) = timed(&h, &point_sql, &points, budget).await;
    assert_eq!(h_point, 1);

    // Raw one-shot UNION ALL over the base tables (no derived structure).
    let raw = names
        .iter()
        .enumerate()
        .map(|(k, nm)| {
            format!(
                "SELECT uri, title FROM ({}) WHERE search_text LIKE ?1",
                branch(k, nm)
            )
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let raw_sql = format!("SELECT * FROM ({raw}) LIMIT 50");
    let (r_none, _) = timed(&h, &raw_sql, &none, budget).await;
    let (r_common, _) = timed(&h, &raw_sql, &common, budget).await;
    // Same rows as the index, in a plain table: separates the matview read
    // path from the row shape.
    let copy_none = match option.as_str() {
        "A" => {
            h.execute_ddl(
                "CREATE TABLE ei_copy (uri TEXT, type TEXT, title TEXT, search_text TEXT)",
            )
            .await
            .expect("copy table");
            h.execute(
                "INSERT INTO ei_copy SELECT uri, type, title, search_text FROM entity_index",
                vec![],
            )
            .await
            .expect("copy rows");
            let copy_sql = "SELECT uri, type, title FROM ei_copy WHERE search_text LIKE ? LIMIT 50";
            timed(&h, copy_sql, &none, budget).await.0
        }
        _ => serde_json::Value::Null,
    };

    let after_writes = writes(&h, &counts, &mut rng, 200).await;
    let (s_back, h_back) = timed(
        &h,
        &backlinks_sql,
        &vec![vec![V::Text(HOT.into())]; 10],
        budget,
    )
    .await;
    assert_eq!(
        h_back,
        HOT_BACKLINKS + 400,
        "backlinks of HOT: seeded plus two write rounds"
    );
    // Maintained, not stale: a written source title shows in the backlinks.
    let fresh = q(
        &h,
        &format!("SELECT * FROM ({backlinks_sql}) WHERE title LIKE 'source edit%'"),
        vec![V::Text(HOT.into())],
    )
    .await;
    assert!(fresh > 0, "backlinks must carry the edited source title");

    let out = serde_json::json!({
        "option": option, "n": n, "types": types, "counts": counts, "seed_ms": seed_ms,
        "create_ms": create, "create_total_ms": create_total,
        "rss_delta_kib": rss1 as i64 - rss0 as i64, "db_delta_bytes": bytes1 - bytes0,
        "writes_before": base_writes, "writes_after": after_writes,
        "search_common": s_common, "search_rare": s_rare, "search_none": s_none,
        "raw_union_none": r_none, "copy_table_none": copy_none, "raw_union_common": r_common,
        "point_by_uri": s_point, "backlinks_hot": s_back,
    });
    println!("S0RESULT {out}");
}
