//! Ingest refusals reach the condition bus through the production disclosure
//! seam and are grouped there: one condition per refusing format, never one per
//! file.
//!
//! @pbt oracle model-equivalence — the refused files per format, kept by a
//!   plain map, against the conditions the bus holds after every step
//! @pbt slips-if-removed a repaired file stays counted in its format's
//!   condition, or the last repair leaves the condition standing

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::IngestRefusals;
use holon_api::RefusedFile;
use holon_app::loro_seams::WritebackDegradedDisclosure;
use holon_filesystem::WritebackDisclosure;
use proptest::prelude::*;

const FILES: [(&str, &str); 6] = [
    ("/vault/a.cook", "cooklang"),
    ("/vault/b.cook", "cooklang"),
    ("/vault/c.cook", "cooklang"),
    ("/vault/d.cook", "cooklang"),
    ("/vault/x.org", "org"),
    ("/vault/y.org", "org"),
];

#[derive(Clone, Debug)]
enum Step {
    Refuse { file: usize, reason: u8 },
    Recover { file: usize },
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (0..FILES.len(), any::<u8>()).prop_map(|(file, reason)| Step::Refuse { file, reason }),
        (0..FILES.len()).prop_map(|file| Step::Recover { file }),
    ]
}

fn seam() -> (Arc<ConditionBus>, WritebackDegradedDisclosure) {
    let bus = Arc::new(ConditionBus::new());
    let disclosure = WritebackDegradedDisclosure { bus: bus.clone() };
    (bus, disclosure)
}

fn refusals(bus: &ConditionBus) -> Vec<Condition> {
    bus.current()
        .into_iter()
        .filter(|c| c.reason.condition_kind() == ConditionKind::VAULT_INGEST_FAILED)
        .collect()
}

/// Refused path → reason, per format: what the bus must disclose.
type Model = BTreeMap<&'static str, BTreeMap<&'static str, String>>;

fn assert_bus_matches(bus: &ConditionBus, model: &Model) {
    let raised = refusals(bus);
    assert_eq!(
        raised.len(),
        model.len(),
        "one condition per format with a refused file: expected formats {:?}, raised subjects {:?}",
        model.keys().collect::<Vec<_>>(),
        raised.iter().map(|c| &c.subject).collect::<Vec<_>>()
    );
    for (format, files) in model {
        let expected: Vec<RefusedFile> = files
            .iter()
            .map(|(path, reason)| RefusedFile {
                path: path.to_string(),
                reason: reason.clone(),
            })
            .collect();
        let condition = raised
            .iter()
            .find(|c| c.subject == *format)
            .unwrap_or_else(|| panic!("no condition for {format}: {raised:?}"));
        let ConditionKind::VaultIngestFailed(refusals) = &condition.reason else {
            unreachable!("filtered to VaultIngestFailed");
        };
        assert_eq!(refusals.format(), *format);
        assert_eq!(
            refusals.count().get(),
            files.len(),
            "{format}: {refusals:?}"
        );
        assert_eq!(
            refusals.examples(),
            &expected[..expected.len().min(IngestRefusals::EXAMPLES)],
            "{format}: the examples are the first refused files by path",
        );
        assert_eq!(
            bus.refused_files(format),
            expected,
            "{format}: refused_files"
        );
    }
}

proptest! {
    #[test]
    fn the_bus_holds_one_condition_per_refusing_format(steps in prop::collection::vec(step(), 1..40)) {
        let (bus, disclosure) = seam();
        let mut model = Model::new();
        for step in steps {
            match step {
                Step::Refuse { file, reason } => {
                    let (path, format) = FILES[file];
                    let reason = format!("reason {reason}");
                    disclosure.ingest_refused(&PathBuf::from(path), format, &reason);
                    model.entry(format).or_default().insert(path, reason);
                }
                Step::Recover { file } => {
                    let (path, format) = FILES[file];
                    disclosure.ingest_recovered(&PathBuf::from(path));
                    if let Some(group) = model.get_mut(format) {
                        group.remove(path);
                        if group.is_empty() {
                            model.remove(format);
                        }
                    }
                }
            }
            assert_bus_matches(&bus, &model);
        }
    }
}

#[test]
fn four_refused_files_of_one_format_are_one_condition() {
    let (bus, disclosure) = seam();
    for (path, format) in &FILES[..4] {
        disclosure.ingest_refused(&PathBuf::from(path), format, "unclosed brace");
    }
    let raised = refusals(&bus);
    assert_eq!(
        raised.len(),
        1,
        "{:?}",
        raised.iter().map(|c| &c.subject).collect::<Vec<_>>()
    );
    let model = Model::from([(
        "cooklang",
        FILES[..4]
            .iter()
            .map(|(path, _)| (*path, "unclosed brace".to_string()))
            .collect(),
    )]);
    assert_bus_matches(&bus, &model);
}

#[test]
fn repairing_one_file_shrinks_the_group_and_the_last_repair_clears_it() {
    let (bus, disclosure) = seam();
    for (path, format) in &FILES[..2] {
        disclosure.ingest_refused(&PathBuf::from(path), format, "unclosed brace");
    }
    assert_eq!(refusals(&bus).len(), 1, "{:?}", refusals(&bus));
    disclosure.ingest_recovered(&PathBuf::from(FILES[0].0));
    assert_eq!(refusals(&bus).len(), 1);
    let model = Model::from([(
        "cooklang",
        BTreeMap::from([(FILES[1].0, "unclosed brace".to_string())]),
    )]);
    assert_bus_matches(&bus, &model);
    disclosure.ingest_recovered(&PathBuf::from(FILES[1].0));
    assert!(refusals(&bus).is_empty(), "{:?}", refusals(&bus));
}
