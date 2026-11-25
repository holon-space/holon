//! A file that vanishes from the vault cascade-deletes its document, and the
//! departures that cascade sends back through the block feed have no file left
//! to reach. Each one is handled in place: none asks for the vault-wide bulk
//! pass and none writes a file, in whatever batches the feed delivers them and
//! while the page registry still lists the deleted page.
//!
//! A page Holon deletes while its file exists still asks for the bulk pass;
//! `a_deleted_page_leaves_no_file` holds that side.

use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockChangeVerdict;
use holon_filesystem::BlockDelta;
use org_vault_double::DOC_ID;
use org_vault_double::Vault;
use org_vault_double::vault;
use proptest::prelude::*;

use crate::org_vault_double;

#[derive(Clone, Debug)]
struct Case {
    /// Sizes of the consecutive feed batches the departures arrive in.
    batches: Vec<usize>,
    /// How many batches reach the controller while the registry still lists
    /// the deleted page.
    registry_lag: usize,
}

/// The departures the cascade sends, leaves before the page.
fn departures(v: &Vault) -> Vec<(EntityUri, BlockDelta)> {
    let page = EntityUri::block(DOC_ID);
    let mut ids: Vec<EntityUri> = v
        .block_ids()
        .iter()
        .map(|id| EntityUri::parse(id).unwrap())
        .collect();
    ids.push(page.clone());
    ids.into_iter()
        .map(|id| (page.clone(), BlockDelta::Remove(id)))
        .collect()
}

fn vault_files(v: &Vault) -> Vec<String> {
    let root = v.path.parent().unwrap();
    let mut files: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| p.strip_prefix(root).unwrap().display().to_string())
        .collect();
    files.sort();
    files
}

async fn run(case: &Case) -> Option<String> {
    let mut v = vault();
    let _ = v.controller.on_file_changed(&v.path).await.unwrap();
    let sent = departures(&v);
    assert_eq!(sent.len(), case.batches.iter().sum::<usize>());

    std::fs::remove_file(&v.path).unwrap();
    let _ = v.controller.on_file_changed(&v.path).await.unwrap();
    let page = EntityUri::block(DOC_ID);
    assert!(
        v.block_ids().is_empty(),
        "the vanished file's cascade left {:?}",
        v.block_ids()
    );

    let mut rest = sent.as_slice();
    for (i, &size) in case.batches.iter().enumerate() {
        if i == case.registry_lag {
            v.docs.by_id.lock().unwrap().remove(&page);
        }
        let (batch, tail) = rest.split_at(size);
        rest = tail;
        for (doc, verdict) in v.controller.on_block_changed_coalesced(batch).await {
            match verdict {
                Ok(BlockChangeVerdict::Handled) => {}
                other => {
                    return Some(format!(
                        "batch {i} {batch:?}: {doc} answered {other:?}, not Handled"
                    ));
                }
            }
        }
    }

    let files = vault_files(&v);
    (!files.is_empty()).then(|| format!("the departures wrote {files:?}"))
}

fn check(case: &Case) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Some(failure) = rt.block_on(run(case)) {
        panic!("{case:?}: {failure}");
    }
}

/// The vault double's document has two headlines under its page.
const DEPARTURES: usize = 3;

fn case_strategy() -> impl Strategy<Value = Case> {
    prop::collection::vec(any::<bool>(), DEPARTURES - 1)
        .prop_map(|cuts| {
            let mut batches = vec![1];
            for cut in cuts {
                match cut {
                    true => batches.push(1),
                    false => *batches.last_mut().unwrap() += 1,
                }
            }
            batches
        })
        .prop_flat_map(|batches| {
            let n = batches.len();
            (Just(batches), 0..=n)
        })
        .prop_map(|(batches, registry_lag)| Case {
            batches,
            registry_lag,
        })
}

/// Cases kept from earlier failures, run through the same property first.
fn fixed_cases() -> Vec<Case> {
    vec![
        // The registry has dropped the page: the departure names no file and
        // asked for the bulk pass.
        Case {
            batches: vec![DEPARTURES],
            registry_lag: 0,
        },
        // The registry still lists the page: its name derives a file again.
        Case {
            batches: vec![1, 1, 1],
            registry_lag: 3,
        },
    ]
}

#[test]
fn a_vanished_file_asks_no_bulk_pass_fixed_cases() {
    for case in fixed_cases() {
        check(&case);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]
    #[test]
    fn a_vanished_file_asks_no_bulk_pass(case in case_strategy()) {
        check(&case);
    }
}
