//! `replace_scope` tombstones the live entities of its scope that the new rows
//! do not name, writes every row, and refuses a row whose id another scope
//! holds. The expected effect is derived from a per-entity read of the store
//! before the call: read each key through `read_entity`, which resolves the
//! root map per entity.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityName;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_core::TypeWriteAuthority;
use holon_core::storage::types::StorageEntity;
use holon_core::type_write_rule::ScopeReplacement;
use holon_loro::loro_document::LoroDocument;
use holon_loro::loro_entity_operations::LoroTypeWriteAuthority;
use holon_loro::loro_entity_operations::StoredEntity;
use holon_loro::loro_entity_operations::entity_root_name;
use holon_loro::loro_entity_operations::read_entity;
use proptest::prelude::*;

const TYPE: &str = "cookware";
const OWNERS: usize = 3;
const IDS: usize = 6;

fn type_def() -> TypeDefinition {
    serde_json::from_value(serde_json::json!({
        "name": TYPE,
        "primary_key": "id",
        "fields": [
            {"name": "id", "sql_type": "TEXT", "primary_key": true},
            {"name": "recipe_id", "sql_type": "TEXT"},
            {"name": "name", "sql_type": "TEXT", "nullable": true},
        ],
    }))
    .expect("the test type parses")
}

fn key_of(n: usize) -> String {
    format!("x{n}")
}

fn owner_of(o: usize) -> String {
    format!("recipe:r{o}")
}

fn scope(owner: usize, ids: &BTreeSet<usize>) -> StorageEntity {
    let rows = ids
        .iter()
        .map(|&n| {
            StorageEntity::from([
                (
                    "id".into(),
                    Value::String(format!("{}:{}", EntityName::new(TYPE).as_str(), key_of(n))),
                ),
                ("recipe_id".into(), Value::String(owner_of(owner))),
                ("name".into(), Value::Null),
            ])
        })
        .collect();
    ScopeReplacement::new("recipe_id".to_string(), owner_of(owner), rows)
        .expect("the scope holds its rows")
        .params()
}

/// Each key's stored state, read one entity at a time.
fn stored(doc: &LoroDocument) -> HashMap<usize, Option<StoredEntity>> {
    let entity = EntityName::new(TYPE);
    let root = entity_root_name(&entity);
    doc.with_read(|d| {
        Ok((0..IDS)
            .map(|n| (n, read_entity(d, &root, &entity, &key_of(n))))
            .collect())
    })
    .unwrap()
}

fn live_owner(stored: &Option<StoredEntity>) -> Option<String> {
    stored
        .as_ref()
        .filter(|s| s.live)
        .and_then(|s| s.fields.get("recipe_id"))
        .and_then(|v| v.as_string().map(str::to_string))
}

fn same(a: &Option<StoredEntity>, b: &Option<StoredEntity>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.live == b.live && a.fields == b.fields,
        _ => false,
    }
}

fn ids() -> impl Strategy<Value = BTreeSet<usize>> {
    prop::collection::btree_set(0..IDS, 0..=IDS)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn a_scope_replacement_has_the_per_entity_expected_effect(
        script in prop::collection::vec((0..OWNERS, ids()), 1..10)
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let doc = Arc::new(LoroDocument::new("global".into()).unwrap());
        let authority = LoroTypeWriteAuthority::new(doc.clone(), None);
        let def = type_def();
        let ops = authority.for_type(&def).unwrap();
        let entity = EntityName::new(TYPE);
        for (owner, named) in script {
            let before = stored(&doc);
            let state_before = doc.with_read(|d| Ok(d.get_deep_value())).unwrap();
            let me = owner_of(owner);
            let refused = named
                .iter()
                .any(|n| live_owner(&before[n]).is_some_and(|held| held != me));

            let result = rt.block_on(ops.execute_operation(
                &entity,
                ScopeReplacement::OP,
                scope(owner, &named),
            ));
            let after = stored(&doc);

            if refused {
                prop_assert!(result.is_err(), "a row held by another scope must be refused");
                let state_after = doc.with_read(|d| Ok(d.get_deep_value())).unwrap();
                prop_assert_eq!(state_before, state_after, "a refused replacement writes nothing");
                continue;
            }
            prop_assert!(result.is_ok(), "{:?}", result.err());
            for n in 0..IDS {
                if named.contains(&n) {
                    prop_assert_eq!(live_owner(&after[&n]), Some(me.clone()), "named key x{} is live in the scope", n);
                } else if live_owner(&before[&n]).as_deref() == Some(me.as_str()) {
                    prop_assert!(after[&n].as_ref().is_some_and(|s| !s.live), "unnamed live key x{} of the scope is tombstoned", n);
                } else {
                    prop_assert!(same(&before[&n], &after[&n]), "key x{} outside the scope is untouched", n);
                }
            }
        }
    }
}
