//! What the shape gate costs one write, reading the pre-write state from the
//! Loro write authority, which has no tagged-neighbourhood read: every plan
//! that is not text-only is simulated.
//!
//! Run in RELEASE: `cargo test -p holon-loro --release --test shape_gate_cost
//! -- --ignored --nocapture`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::Value;
use holon_core::ShapeValidators;
use holon_core::WriteAuthorityReads;
use holon_core::shape_gate::PlanOp;
use holon_core::shape_gate::judge_plan;
use holon_core::storage::types::StorageEntity;
use holon_loro::LoroBlockOperations;
use holon_loro::LoroDocumentStore;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_document_store::DocScope;
use tokio::sync::RwLock;

const SIBLINGS: usize = 2000;
const SAMPLES: usize = 300;
/// Tagged decisions on the page the measured writes sit on, three options
/// each.
const DECISIONS: [usize; 3] = [1, 10, 100];

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

async fn create(
    backend: &LoroBackend,
    parent: EntityUri,
    id: &str,
    properties: &[(&str, &str)],
    tags: &[&str],
) {
    let properties: HashMap<String, Value> = properties
        .iter()
        .map(|(k, v)| (k.to_string(), s(v)))
        .collect();
    let edges = BlockEdges {
        tags: Tags::from(tags.iter().map(|t| t.to_string()).collect::<Vec<_>>()),
        ..BlockEdges::default()
    };
    backend
        .create_block_with_properties(
            parent,
            BlockContent::text("a note"),
            Some(EntityUri::block(id)),
            &properties,
            &edges,
        )
        .await
        .expect("create");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn gate_cost_on_a_plain_block_over_the_loro_authority() {
    let dir = tempfile::tempdir().unwrap();
    let store = LoroDocumentStore::new(dir.path().to_path_buf()).with_peer_id(Some(7));
    let backend = LoroBackend::from_document(store.get_doc(DocScope::Global).await.unwrap());
    create(&backend, EntityUri::no_parent(), "page", &[], &[]).await;
    for i in 0..SIBLINGS {
        create(
            &backend,
            EntityUri::block("page"),
            &format!("n{i}"),
            &[],
            &[],
        )
        .await;
    }
    let authority: Arc<dyn WriteAuthorityReads> = Arc::new(LoroBlockOperations::new(Arc::new(
        RwLock::new(store.clone()),
    )));
    assert!(
        authority
            .tagged_neighbourhood(&["decision"], &[], None)
            .await
            .unwrap()
            .is_none(),
        "this measures the simulating path"
    );
    let validators = ShapeValidators::registered();
    let mut decisions = 0;
    for target in DECISIONS {
        while decisions < target {
            let id = format!("d{decisions}");
            create(
                &backend,
                EntityUri::block("page"),
                &id,
                &[("task_state", "?")],
                &["decision"],
            )
            .await;
            for key in ["a", "b", "c"] {
                create(
                    &backend,
                    EntityUri::block(&id),
                    &format!("{id}-{key}"),
                    &[("option", key)],
                    &[],
                )
                .await;
            }
            decisions += 1;
        }
        for (name, op_name, p) in &cases() {
            let ops = [PlanOp { op_name, params: p }];
            let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                let t = Instant::now();
                let judged = judge_plan(&validators, authority.clone(), &ops)
                    .await
                    .expect("reads");
                samples.push(t.elapsed());
                judged.verdict.expect("every case is a legal write");
            }
            samples.sort();
            eprintln!(
                "[shape-gate cost/loro] decisions={decisions} {name}: p50={:?} p95={:?} max={:?} \
                 ({SAMPLES} samples) load: {}",
                samples[SAMPLES / 2],
                samples[SAMPLES * 95 / 100],
                samples[SAMPLES - 1],
                load_average()
            );
        }
    }
}

/// The machine's load averages, as `uptime` prints them.
fn load_average() -> String {
    let out = std::process::Command::new("uptime")
        .output()
        .expect("uptime runs");
    let text = String::from_utf8(out.stdout).expect("uptime prints text");
    let at = text
        .find("load average")
        .unwrap_or_else(|| panic!("uptime printed no load average: {text}"));
    text[at..].trim().to_string()
}

fn cases() -> Vec<(&'static str, &'static str, StorageEntity)> {
    vec![
        (
            "set_field(content), text-only",
            "set_field",
            params(&[
                ("id", s("block:n1000")),
                ("field", s("content")),
                ("value", s("x")),
            ]),
        ),
        (
            "set_field(task_state), not tagged",
            "set_field",
            params(&[
                ("id", s("block:n1000")),
                ("field", s("task_state")),
                ("value", s("TODO")),
            ]),
        ),
        (
            "indent in a 2000-sibling list, not tagged",
            "indent",
            params(&[("id", s("block:n1000"))]),
        ),
        (
            "set_field(choose) on a decision",
            "set_field",
            params(&[
                ("id", s("block:d0")),
                ("field", s("choose")),
                ("value", s("1")),
            ]),
        ),
    ]
}
