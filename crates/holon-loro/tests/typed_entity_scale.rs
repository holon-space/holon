//! Increment 1 of the Loro typed-entity store: what the candidate shapes cost
//! at 10,000 entities × 8 fields — snapshot bytes, import time, the latency
//! from one write to its changed `(type, id)` in the `subscribe_root` event,
//! and the no-Turso read side (point read, type scan, field filter, one guard
//! over that filter).
//!
//! The budgets are the plan's falsifiers: a write must reach the changed-id set
//! in under 5 ms, and a guard must stay inside the p95 < 200 ms interaction
//! budget.
//!
//! RELEASE profile, and every number it prints names it:
//! `cargo test -p holon-loro --release --test typed_entity_scale --
//! --ignored --nocapture`
//!
//! `shape_cost_at_small_scale` is the same body at 500 entities with no
//! budgets, so the measurement code is exercised by the default test run.

mod typed_entity_shape;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use holon_loro::WriteOrigin;
use holon_loro::loro_document::LoroDocument;
use loro::ExportMode;
use loro::LoroDoc;
use loro::LoroValue;
use typed_entity_shape::CANDIDATES;
use typed_entity_shape::FieldPath;
use typed_entity_shape::Shape;
use typed_entity_shape::TOMBSTONE;
use typed_entity_shape::changed_entities;
use typed_entity_shape::filter_by_field;
use typed_entity_shape::put_field;
use typed_entity_shape::read_entity;
use typed_entity_shape::read_entity_declared;
use typed_entity_shape::read_field;
use typed_entity_shape::scan_ids;

const TYPE: &str = "repair-record";
const ORIGIN: WriteOrigin = WriteOrigin::Probe("typed_entity_scale");
/// Entities per commit while seeding.
const BATCH: usize = 500;
/// Share of entities with `severity = "critical"`, the guard's filter.
const CRITICAL_EVERY: usize = 100;

const PROFILE: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

fn id_of(i: usize) -> String {
    format!("rec-{i:06}")
}

/// The eight persisted fields of the measured type: six scalars and two
/// overflow-bag keys.
fn fields_of(i: usize) -> [(FieldPath, LoroValue); 8] {
    let severity = if i % CRITICAL_EVERY == 0 {
        "critical"
    } else if i.is_multiple_of(2) {
        "high"
    } else {
        "low"
    };
    [
        (
            FieldPath::scalar("subject"),
            LoroValue::from(format!("block:{i:08}")),
        ),
        (FieldPath::scalar("severity"), LoroValue::from(severity)),
        (
            FieldPath::scalar("acked"),
            LoroValue::from(!i.is_multiple_of(7)),
        ),
        (
            FieldPath::scalar("device"),
            LoroValue::from(format!("device-{}", i % 3)),
        ),
        (
            FieldPath::scalar("created_at"),
            LoroValue::from(1_760_000_000i64 + i as i64),
        ),
        (FieldPath::scalar(TOMBSTONE), LoroValue::Null),
        (
            FieldPath::property("color"),
            LoroValue::from(if i.is_multiple_of(2) { "red" } else { "amber" }),
        ),
        (
            FieldPath::property("origin"),
            LoroValue::from(format!("rule-{}", i % 11)),
        ),
    ]
}

/// Seed through the production write seam (`LoroDocument::with_write`), the
/// same one the projector's `subscribe_root` leg observes.
fn seed(shape: Shape, entities: usize) -> anyhow::Result<(Arc<LoroDocument>, Duration)> {
    let doc = Arc::new(LoroDocument::new("global".to_string())?);
    let start = Instant::now();
    for chunk in (0..entities).collect::<Vec<_>>().chunks(BATCH) {
        doc.with_write(ORIGIN, |txn| {
            for &i in chunk {
                let id = id_of(i);
                for (field, value) in fields_of(i) {
                    put_field(txn, shape, TYPE, &id, &field, value);
                }
            }
            Ok(())
        })?;
    }
    Ok((doc, start.elapsed()))
}

struct Quantiles {
    p50: Duration,
    p95: Duration,
    max: Duration,
}

fn quantiles(mut samples: Vec<Duration>) -> Quantiles {
    assert!(!samples.is_empty(), "a measurement needs samples");
    samples.sort();
    let at = |q: f64| samples[((samples.len() as f64 * q) as usize).min(samples.len() - 1)];
    Quantiles {
        p50: at(0.5),
        p95: at(0.95),
        max: samples[samples.len() - 1],
    }
}

fn report(label: &str, q: &Quantiles) {
    println!(
        "  {label:<46} p50={:>10.1?} p95={:>10.1?} max={:>10.1?}  [{PROFILE}]",
        q.p50, q.p95, q.max
    );
}

/// A plausible no-Turso guard: "every critical record is acked". It runs the
/// field filter and then a point read of `acked` per match — no IVM anywhere.
fn guard_every_critical_is_acked(doc: &LoroDoc, shape: Shape) -> bool {
    let critical = filter_by_field(
        doc,
        shape,
        TYPE,
        &FieldPath::scalar("severity"),
        &LoroValue::from("critical"),
    );
    assert!(!critical.is_empty(), "the guard's filter must match rows");
    critical.iter().all(|id| {
        read_field(doc, shape, TYPE, id, &FieldPath::scalar("acked")) == Some(LoroValue::from(true))
    })
}

/// Time from the write to the changed `(type, id)` set being in hand: the
/// commit through the production write seam, the `subscribe_root` callback
/// that extracts the pair with no doc access, and the drain the projector's
/// entity leg would do.
type Sink = Arc<Mutex<BTreeSet<(String, String)>>>;

/// Subscribe the entity leg and prove it delivers, before anything is timed.
/// Loro decides whether to record events when a transaction opens, so the
/// subscription goes up before the reads and one write is spent warming it.
fn arm(doc: &LoroDocument, shape: Shape) -> anyhow::Result<(Sink, loro::Subscription)> {
    let collected: Sink = Arc::default();
    let sink = collected.clone();
    let sub = doc.doc().subscribe_root(Arc::new(move |event| {
        sink.lock()
            .expect("the fact sink is not poisoned")
            .extend(changed_entities(&event));
    }));
    // The commit that opened before the subscription records nothing, so the
    // warm-up spends two.
    for warm in ["warm-1", "warm-2"] {
        doc.with_write(ORIGIN, |txn| {
            put_field(
                txn,
                shape,
                TYPE,
                &id_of(0),
                &FieldPath::scalar("device"),
                LoroValue::from(warm),
            );
            Ok(())
        })?;
    }
    let warmed = std::mem::take(&mut *collected.lock().expect("the fact sink is not poisoned"));
    assert!(
        warmed.contains(&(TYPE.to_string(), id_of(0))),
        "the subscription must deliver the warm-up write; got {warmed:?}"
    );
    Ok((collected, sub))
}

fn write_to_changed_id(
    doc: &LoroDocument,
    collected: &Sink,
    shape: Shape,
    entities: usize,
    samples: usize,
) -> anyhow::Result<Quantiles> {
    let mut out = Vec::with_capacity(samples);
    for k in 0..samples {
        let id = id_of((k * 7919) % entities);
        let start = Instant::now();
        doc.with_write(ORIGIN, |txn| {
            put_field(
                txn,
                shape,
                TYPE,
                &id,
                &FieldPath::scalar("severity"),
                LoroValue::from(format!("churn-{k}")),
            );
            Ok(())
        })?;
        let changed =
            std::mem::take(&mut *collected.lock().expect("the fact sink is not poisoned"));
        out.push(start.elapsed());
        assert_eq!(
            changed,
            BTreeSet::from([(TYPE.to_string(), id.clone())]),
            "one write must name exactly its own entity"
        );
    }
    Ok(quantiles(out))
}

fn measure(shape: Shape, entities: usize, samples: usize, budgets: bool) -> anyhow::Result<()> {
    let (doc, seed_time) = seed(shape, entities)?;
    let (collected, _sub) = arm(&doc, shape)?;
    println!(
        "\n== {} at {entities} entities × 8 fields  (seeded in {seed_time:.1?}) [{PROFILE}]",
        shape.label()
    );

    let snapshot = doc.with_read(|d| Ok(d.export(ExportMode::Snapshot)?))?;
    let mut imports = Vec::new();
    for _ in 0..5 {
        let fresh = LoroDoc::new();
        let start = Instant::now();
        fresh.import(&snapshot).expect("snapshot import");
        imports.push(start.elapsed());
        assert_eq!(
            scan_ids(&fresh, shape, TYPE).len(),
            entities,
            "the imported doc must hold every entity"
        );
    }
    let import = quantiles(imports);
    println!(
        "  {:<46} {} bytes ({:.1} B/entity)  [{PROFILE}]",
        "snapshot size",
        snapshot.len(),
        snapshot.len() as f64 / entities as f64
    );
    report("snapshot import", &import);

    let declared: Vec<FieldPath> = fields_of(0).into_iter().map(|(f, _)| f).collect();
    let mut point: Vec<Duration> = Vec::new();
    for k in 0..samples {
        point.push({
            let id = id_of((k * 7919) % entities);
            let start = Instant::now();
            let got =
                doc.with_read(|d| Ok(read_entity_declared(d, shape, TYPE, &id, &declared)))?;
            let took = start.elapsed();
            assert_eq!(
                got.len(),
                8,
                "a point read returns all eight declared fields"
            );
            took
        });
    }
    report("point read by id (8 declared fields)", &quantiles(point));

    let mut discovery: Vec<Duration> = Vec::new();
    for k in 0..samples {
        let id = id_of((k * 7919) % entities);
        let start = Instant::now();
        let got = doc.with_read(|d| Ok(read_entity(d, shape, TYPE, &id)))?;
        discovery.push(start.elapsed());
        assert_eq!(
            got.expect("a seeded entity is readable").len(),
            8,
            "a discovery read returns all eight keys"
        );
    }
    report("point read by id (field discovery)", &quantiles(discovery));

    let mut scans: Vec<Duration> = Vec::new();
    for _ in 0..20 {
        scans.push({
            let start = Instant::now();
            let ids = doc.with_read(|d| Ok(scan_ids(d, shape, TYPE)))?;
            let took = start.elapsed();
            assert_eq!(ids.len(), entities, "a type scan returns every id");
            took
        });
    }
    report("type scan (ids only)", &quantiles(scans));

    let mut filters: Vec<Duration> = Vec::new();
    for _ in 0..20 {
        filters.push({
            let start = Instant::now();
            let hits = doc.with_read(|d| {
                Ok(filter_by_field(
                    d,
                    shape,
                    TYPE,
                    &FieldPath::scalar("severity"),
                    &LoroValue::from("high"),
                ))
            })?;
            let took = start.elapsed();
            assert!(!hits.is_empty(), "the filter must match rows");
            took
        });
    }
    report("field filter (severity = high, ~50 %)", &quantiles(filters));

    let mut guards: Vec<Duration> = Vec::new();
    for _ in 0..20 {
        guards.push({
            let start = Instant::now();
            let held = doc.with_read(|d| Ok(guard_every_critical_is_acked(d, shape)))?;
            let took = start.elapsed();
            assert!(!held, "the seed leaves an unacked critical record");
            took
        });
    }
    let guard = quantiles(guards);
    report("guard: every critical record is acked", &guard);

    let write = write_to_changed_id(&doc, &collected, shape, entities, samples)?;
    report("write -> changed (type, id)", &write);

    if budgets {
        assert!(
            guard.p95 < Duration::from_millis(200),
            "{}: the guard's p95 {:?} breaks the 200 ms interaction budget ({PROFILE})",
            shape.label(),
            guard.p95
        );
        assert!(
            write.p95 < Duration::from_millis(5),
            "{}: write -> changed-id p95 {:?} breaks the 5 ms falsifier ({PROFILE})",
            shape.label(),
            write.p95
        );
    }
    Ok(())
}

#[test]
#[ignore = "release bench: `cargo test -p holon-loro --release --test typed_entity_scale -- --ignored --nocapture`"]
fn shape_cost_at_ten_thousand_entities() -> anyhow::Result<()> {
    assert!(
        !cfg!(debug_assertions),
        "the budgets are release numbers; run this with --release"
    );
    for shape in CANDIDATES {
        measure(shape, 10_000, 300, true)?;
    }
    Ok(())
}

#[test]
fn shape_cost_at_small_scale() -> anyhow::Result<()> {
    for shape in CANDIDATES {
        measure(shape, 500, 50, false)?;
    }
    Ok(())
}
