//! Spike S0 option C (throwaway): the shared entity index as one view in the
//! differential-dataflow engine. Drives `lower::Dataflow` directly with an
//! entity catalog, because `ViewEngine` hard-codes the blocks/focus_roots
//! catalog and is fed only `SnapshotBlock`s. `S0_N`, `S0_TYPES` set the
//! corpus; prints one `S0RESULT {json}` line.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_views::lower::Dataflow;
use holon_views::plan::Catalog;
use holon_views::plan::Col;
use holon_views::plan::ColType;
use holon_views::plan::Expr;
use holon_views::plan::Plan;
use holon_views::plan::RelationId;
use holon_views::plan::Schema;
use holon_views::plan::check_all;
use holon_views::row::Datum;
use holon_views::row::DynRow;
use holon_views::row::Id;
use holon_views::row::Row;
use timely::WorkerConfig;
use timely::communication::Allocator;
use timely::communication::allocator::Thread;
use timely::worker::Worker;

const ENTITIES: RelationId = RelationId(0);
const LINKS: RelationId = RelationId(1);
const HOT: u32 = u32::MAX;
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
        .unwrap()
}

fn rss_kib() -> i64 {
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
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |p: f64| v[((v.len() as f64 - 1.0) * p).round() as usize];
    serde_json::json!({"n": v.len(), "p50": at(0.5), "p95": at(0.95), "max": v[v.len()-1]})
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// `(uri, type, title, search_text)`
fn entity(uri: u32, ty: &Arc<str>, title: &str, text: &str) -> DynRow {
    DynRow::build(
        &(),
        [
            Datum::Id(Id(uri)),
            Datum::Text(ty.clone()),
            Datum::Text(title.into()),
            Datum::Text(text.into()),
        ],
    )
}

/// `(source, target)`
fn link(source: u32, target: u32) -> DynRow {
    DynRow::build(&(), [Datum::Id(Id(source)), Datum::Id(Id(target))])
}

fn text(row: &DynRow, c: u16) -> Arc<str> {
    match row.get(&(), Col(c)) {
        Datum::Text(t) => t,
        other => panic!("{other:?}"),
    }
}

fn id(row: &DynRow, c: u16) -> u32 {
    match row.get(&(), Col(c)) {
        Datum::Id(Id(i)) => i,
        other => panic!("{other:?}"),
    }
}

/// The consumer's read model: the index by uri, and backlinks by target.
#[derive(Default)]
struct Mirror {
    index: HashMap<u32, DynRow>,
    backlinks: HashMap<u32, BTreeSet<DynRow>>,
}

struct Host {
    worker: Worker,
    flow: Dataflow<DynRow>,
    mirror: Mirror,
}

impl Host {
    fn new() -> Host {
        let mut worker = Worker::new(
            WorkerConfig::default(),
            Allocator::Thread(Thread::default()),
            None,
        );
        let catalog = Catalog {
            relations: vec![
                Schema(vec![
                    ColType::Id,
                    ColType::Text,
                    ColType::Text,
                    ColType::Text,
                ]),
                Schema(vec![ColType::Id, ColType::Id]),
            ],
        };
        let col = |c: u16| Expr::Col(Col(c));
        let entities = Plan::scan(ENTITIES);
        let index = entities.project(vec![col(0), col(1), col(2), col(3)]);
        // links ++ entities: (source, target, uri, type, title, text)
        let backlinks = Plan::scan(LINKS)
            .join(&entities, vec![(Col(0), Col(0))])
            .project(vec![col(1), col(0), col(4), col(3)]);
        let plans = check_all(&[index, backlinks], &catalog).expect("well typed");
        let flow = Dataflow::build(&mut worker, &catalog, &plans);
        Host {
            worker,
            flow,
            mirror: Mirror::default(),
        }
    }

    fn commit(&mut self) {
        self.flow.commit(&mut self.worker).expect("commit");
        for (row, diff) in self.flow.take_changes(0).expect("index") {
            assert!(diff == 1 || diff == -1, "set semantics: {diff}");
            let uri = id(&row, 0);
            if diff > 0 {
                self.mirror.index.insert(uri, row);
            } else if self.mirror.index.get(&uri) == Some(&row) {
                self.mirror.index.remove(&uri);
            }
        }
        for (row, diff) in self.flow.take_changes(1).expect("backlinks") {
            let set = self.mirror.backlinks.entry(id(&row, 0)).or_default();
            if diff > 0 {
                set.insert(row);
            } else {
                assert!(
                    set.remove(&row),
                    "a retraction of a row the mirror never saw"
                );
            }
        }
    }

    fn search(&self, needle: &str) -> usize {
        self.mirror
            .index
            .values()
            .filter(|r| text(r, 3).to_lowercase().contains(needle))
            .take(50)
            .count()
    }
}

#[test]
#[ignore = "S0 timing instrument; driven by the scratchpad capabilities/run-s0.sh"]
fn entity_index_dd_spike() {
    let n = env_usize("S0_N");
    let types = env_usize("S0_TYPES");
    let names: Vec<Arc<str>> = (0..types)
        .map(|k| {
            Arc::from(if k == 0 {
                "block".to_string()
            } else {
                format!("t{k}")
            })
        })
        .collect();
    let mut rng = Lcg(42);
    let mut rows: Vec<DynRow> = (0..n as u32)
        .map(|i| {
            let ty = &names[i as usize % types];
            let t = rng.text(i as usize, 4);
            entity(i, ty, &t, &format!("{t} {}", rng.text(i as usize, 6)))
        })
        .collect();
    rows.push(entity(HOT, &names[0], "hot target", "hot target"));
    let mut links: Vec<DynRow> = (0..n / 2)
        .map(|_| link(rng.below(n) as u32, rng.below(n) as u32))
        .collect();
    links.extend((0..HOT_BACKLINKS as u32).map(|s| link(s, HOT)));
    links.sort();
    links.dedup();

    let rss0 = rss_kib();
    let t = Instant::now();
    let mut host = Host::new();
    for r in &rows {
        host.flow.update(ENTITIES, r.clone(), 1);
    }
    for l in &links {
        host.flow.update(LINKS, l.clone(), 1);
    }
    host.commit();
    let create_ms = ms(t.elapsed());
    let rss1 = rss_kib();
    assert_eq!(host.mirror.index.len(), n + 1);
    assert_eq!(host.mirror.backlinks[&HOT].len(), HOT_BACKLINKS);

    let mut write = vec![];
    for r in 0..200 {
        let i = rng.below(n);
        let old = rows[i].clone();
        let t = rng.text(i, 4);
        let new = entity(
            i as u32,
            &text(&old, 1),
            &format!("{t} edit{r}"),
            &format!("{t} edit{r}"),
        );
        let start = Instant::now();
        host.flow.update(ENTITIES, old, -1);
        host.flow.update(ENTITIES, new.clone(), 1);
        host.commit();
        write.push(ms(start.elapsed()));
        rows[i] = new;
    }
    // A source of a HOT backlink: the edit flows through the join.
    let mut source = vec![];
    for r in 0..200 {
        let i = r % HOT_BACKLINKS;
        let old = rows[i].clone();
        let new = entity(
            i as u32,
            &text(&old, 1),
            &format!("source edit {r}"),
            "source edit",
        );
        let start = Instant::now();
        host.flow.update(ENTITIES, old, -1);
        host.flow.update(ENTITIES, new.clone(), 1);
        host.commit();
        source.push(ms(start.elapsed()));
        rows[i] = new;
    }
    let mut link_insert = vec![];
    for r in 0..200u32 {
        let start = Instant::now();
        host.flow
            .update(LINKS, link(HOT_BACKLINKS as u32 + r, HOT), 1);
        host.commit();
        link_insert.push(ms(start.elapsed()));
    }
    let hot = &host.mirror.backlinks[&HOT];
    assert_eq!(hot.len(), HOT_BACKLINKS + 200);
    assert!(
        hot.iter().any(|r| text(r, 2).starts_with("source edit")),
        "backlinks carry the edited title"
    );

    let timed = |needles: Vec<String>, expect: Option<usize>| {
        let mut v = vec![];
        for _ in 0..3 {
            for nd in &needles {
                let s = Instant::now();
                let hits = host.search(nd);
                v.push(ms(s.elapsed()));
                if let Some(e) = expect {
                    assert_eq!(hits, e, "{nd}");
                }
            }
        }
        pct(v)
    };
    let common = timed(
        WORDS.iter().take(10).map(|w| w.to_string()).collect(),
        Some(50),
    );
    let rare = timed(
        (0..10).map(|i| format!("tok{}x", 7 * i + 3)).collect(),
        None,
    );
    let none = timed((0..10).map(|i| format!("zzq{i}")).collect(), Some(0));
    let mut point = vec![];
    for i in 0..60u32 {
        let s = Instant::now();
        let hit = host
            .mirror
            .index
            .get(&(i * 37 % n as u32))
            .map(|r| text(r, 2));
        point.push(ms(s.elapsed()));
        assert!(hit.is_some());
    }
    let mut back = vec![];
    for _ in 0..30 {
        let s = Instant::now();
        let k = host.mirror.backlinks[&HOT]
            .iter()
            .map(|r| text(r, 2))
            .count();
        back.push(ms(s.elapsed()));
        assert_eq!(k, HOT_BACKLINKS + 200);
    }

    let out = serde_json::json!({
        "option": "C", "n": n, "types": types, "links": links.len(),
        "create_ms": create_ms, "rss_delta_kib": rss1 - rss0, "arrangements": host.flow.arrangements(),
        "write_entity": pct(write), "write_backlink_source": pct(source), "link_insert": pct(link_insert),
        "search_common": common, "search_rare": rare, "search_none": none,
        "point_by_uri": pct(point), "backlinks_hot": pct(back),
    });
    println!("S0RESULT {out}");
}
