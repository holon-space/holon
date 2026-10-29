//! Two peers that hold the same operations show the same document state, also
//! when only one of them read or replaced a scope of a type that holds nothing.
//! A Loro root a peer only looks at has no operation to replicate, so it must
//! not appear in that peer's state alone.

use std::sync::Arc;

use holon_api::EntityName;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_core::TypeWriteAuthority;
use holon_core::storage::types::StorageEntity;
use holon_core::type_write_rule::ScopeReplacement;
use holon_loro::loro_document::LoroDocument;
use holon_loro::loro_entity_operations::LoroTypeWriteAuthority;
use loro::ExportMode;
use proptest::prelude::*;

const TYPES: [&str; 2] = ["cookware", "recipe_timer"];
const OWNERS: [&str; 2] = ["recipe:r0", "recipe:r1"];

fn type_def(name: &str) -> TypeDefinition {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "primary_key": "id",
        "fields": [
            {"name": "id", "sql_type": "TEXT", "primary_key": true},
            {"name": "recipe_id", "sql_type": "TEXT"},
            {"name": "name", "sql_type": "TEXT", "nullable": true},
        ],
    }))
    .expect("the test type parses")
}

#[derive(Clone, Debug)]
enum Step {
    Replace {
        peer: usize,
        ty: usize,
        owner: usize,
        rows: u8,
    },
    Read {
        peer: usize,
        ty: usize,
    },
    ReadAll {
        peer: usize,
        ty: usize,
    },
    Sync,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (0..2usize, 0..2usize, 0..2usize, 0..3u8).prop_map(|(peer, ty, owner, rows)| {
            Step::Replace {
                peer,
                ty,
                owner,
                rows,
            }
        }),
        (0..2usize, 0..2usize).prop_map(|(peer, ty)| Step::Read { peer, ty }),
        (0..2usize, 0..2usize).prop_map(|(peer, ty)| Step::ReadAll { peer, ty }),
        Just(Step::Sync),
    ]
}

/// The id of a row of `ty`: entity references carry the entity's scheme.
fn row_id(ty: &str, owner: &str, n: u8) -> String {
    format!("{}:{owner}-{n}", EntityName::new(ty).as_str())
}

fn scope(ty: &str, owner: &str, rows: u8) -> StorageEntity {
    let rows = (0..rows)
        .map(|n| {
            StorageEntity::from([
                ("id".into(), Value::String(row_id(ty, owner, n))),
                ("recipe_id".into(), Value::String(owner.to_string())),
                ("name".into(), Value::Null),
            ])
        })
        .collect();
    ScopeReplacement::new("recipe_id".to_string(), owner.to_string(), rows)
        .expect("the scope holds its rows")
        .params()
}

/// Hands each peer the other's missing operations, as a transport does.
fn sync(a: &LoroDocument, b: &LoroDocument) {
    send(a, b);
    send(b, a);
}

fn send(from: &LoroDocument, to: &LoroDocument) {
    let known = to.with_read(|d| Ok(d.oplog_vv())).unwrap();
    let update = from
        .with_read(|d| Ok(d.export(ExportMode::updates(&known))?))
        .unwrap();
    to.apply_update(&update).unwrap();
}

fn state(doc: &LoroDocument) -> loro::LoroValue {
    doc.with_read(|d| Ok(d.get_deep_value())).unwrap()
}

async fn run(script: &[Step]) {
    let peers: Vec<Arc<LoroDocument>> = (1..=2)
        .map(|p| Arc::new(LoroDocument::new_with_peer_id("global".into(), Some(p)).unwrap()))
        .collect();
    let authorities: Vec<LoroTypeWriteAuthority> = peers
        .iter()
        .map(|doc| LoroTypeWriteAuthority::new(doc.clone(), None))
        .collect();
    for step in script {
        match *step {
            Step::Replace {
                peer,
                ty,
                owner,
                rows,
            } => {
                let def = type_def(TYPES[ty]);
                authorities[peer]
                    .for_type(&def)
                    .unwrap()
                    .execute_operation(
                        &EntityName::new(TYPES[ty]),
                        ScopeReplacement::OP,
                        scope(TYPES[ty], OWNERS[owner], rows),
                    )
                    .await
                    .unwrap();
            }
            Step::Read { peer, ty } => {
                let def = type_def(TYPES[ty]);
                authorities[peer]
                    .read(&def, &row_id(TYPES[ty], OWNERS[0], 0))
                    .await
                    .unwrap();
            }
            Step::ReadAll { peer, ty } => {
                authorities[peer]
                    .read_all(&type_def(TYPES[ty]))
                    .await
                    .unwrap();
            }
            Step::Sync => sync(&peers[0], &peers[1]),
        }
    }
    sync(&peers[0], &peers[1]);
    assert_eq!(
        state(&peers[0]),
        state(&peers[1]),
        "two peers with the same operations show different states after {script:?}"
    );
}

fn block_on(script: &[Step]) {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(run(script));
}

#[test]
fn an_empty_scope_replacement_on_one_peer_leaves_both_states_equal() {
    block_on(&[Step::Replace {
        peer: 0,
        ty: 0,
        owner: 0,
        rows: 0,
    }]);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn peers_with_the_same_operations_show_the_same_state(
        script in prop::collection::vec(step(), 1..12)
    ) {
        block_on(&script);
    }
}
