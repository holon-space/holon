//! A type declared at runtime survives a restart: the next boot declares it
//! again through the admission seat, so its stored rows take writes. A stored
//! definition the seat refuses does not stop the boot or the other types; the
//! boot report names it and why.
//!
//! @pbt kind harness
//! @pbt covers runtime-type-redeclared-at-boot — a declared type takes writes
//! again after a restart, with its rows kept, under the Loro write authority
//! @pbt covers refused-stored-type-disclosed — a stored definition the seat
//! refuses at boot is a failed boot step naming the type and the refusal
//! @pbt covers unreadable-stored-type-refused-alone — one stored entry that is
//! not a TypeDefinition (SQL row or Loro map value) is one named refusal in the
//! boot report, merged with the seat's refusals in key order; every readable
//! type the seat admits is still declared and takes writes, and the entry stays
//! stored, under both write authorities
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone cannot store a
//! definition the seat refuses, and its typed reboot cases draw SQL authority

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_api::EntityName;
use holon_api::FieldSchema;
use holon_api::HomeProfileId;
use holon_api::StorageEntity;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_frontend::platform::BootStep;
use holon_loro::foreign_root::ForeignRootKind;
use holon_loro::foreign_root::ForeignRootValue;
use holon_loro::foreign_root::foreign_root_kind;
use holon_loro::loro_entity_operations::TYPE_DECLARATIONS_ROOT;
use proptest::prelude::*;

struct Booted {
    engine: Arc<BackendEngine>,
    injector: fluxdi::Injector,
    session: Arc<FrontendSession>,
}

async fn boot(dir: &Path, crdt: bool) -> Booted {
    let config = HolonConfig {
        crdt: holon_frontend::config::CrdtPreferences {
            enabled: Some(crdt),
            ..Default::default()
        },
        db_path: Some(dir.join("holon.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(
            holon_api::UiInfo::permissive(),
            std::sync::Arc::new(holon_api::SystemClock),
        ),
        dir.to_path_buf(),
        HashSet::new(),
        Arc::new(holon_conditions::ConditionBus::new()),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |injector| injector.clone(),
    )
    .await
    .expect("the shared wiring must boot a session");
    Booted {
        engine,
        injector,
        session,
    }
}

fn params(pairs: &[(&str, &str)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), Value::String((*v).into())))
        .collect()
}

async fn dispatch(
    booted: &Booted,
    entity: &str,
    op: &str,
    p: StorageEntity,
) -> holon_core::Result<()> {
    booted
        .engine
        .get_dispatcher()
        .execute_operation(&EntityName::new(entity), op, p)
        .await
        .map(|_| ())
}

async fn authority(booted: &Booted) -> Arc<dyn holon_core::TypeWriteAuthority> {
    booted
        .injector
        .resolve_async::<dyn holon_core::TypeWriteAuthority>()
        .await
}

fn kept_type() -> TypeDefinition {
    named_type("gen_kept")
}

fn named_type(name: &str) -> TypeDefinition {
    TypeDefinition::new(
        name,
        [
            FieldSchema::new("id", "TEXT").primary_key(),
            FieldSchema::new("title", "TEXT"),
        ]
        .into_iter()
        .chain(FieldSchema::overflow_pair())
        .collect(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Corruption {
    InvalidJson,
    /// Valid JSON of a TypeDefinition that lost a required field.
    OldShape,
    Empty,
    /// A Loro map value that is not a string.
    NonString,
    /// A root of another kind under the name of the type declarations map, as
    /// a peer may write one: emptied or not, written before or after the
    /// map's entries.
    ForeignRoot {
        kind: ForeignRootKind,
        emptied: bool,
        first: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Home {
    SqlTable,
    LoroMap,
}

#[derive(Clone, Debug)]
struct Case {
    crdt: bool,
    valid: usize,
    unreadable: Vec<(Corruption, Home)>,
    /// Readable definitions stored past the seat that the seat refuses at boot.
    seat_refused: usize,
}

fn corrupt_text(corruption: Corruption) -> String {
    match corruption {
        Corruption::InvalidJson => "{not json".to_string(),
        Corruption::OldShape => {
            let mut json = serde_json::to_value(kept_type()).expect("serialize");
            json.as_object_mut().expect("an object").remove("name");
            let text = json.to_string();
            assert!(
                serde_json::from_str::<TypeDefinition>(&text).is_err(),
                "the old shape must stay unreadable: {text}"
            );
            text
        }
        Corruption::Empty => String::new(),
        Corruption::NonString | Corruption::ForeignRoot { .. } => unreachable!("not text"),
    }
}

fn valid_name(i: usize) -> String {
    format!("gen_valid{i}")
}

fn row_id(i: usize, n: usize) -> String {
    format!("{}:k{n}", valid_name(i).replace('_', "-"))
}

fn bad_name(i: usize) -> String {
    format!("gen_bad{i}")
}

/// Sorts before every `bad_name`, so an unsorted merge of the two refusal
/// kinds is out of key order.
fn seat_name(i: usize) -> String {
    format!("gen_aseat{i}")
}

fn seat_refused_type(i: usize) -> TypeDefinition {
    let mut def = named_type(&seat_name(i));
    def.home = Some(HomeProfileId::parse("holon-nativ").expect("well-formed, merely wrong"));
    def
}

async fn plant(booted: &Booted, key: &str, corruption: Corruption, home: Home) {
    match home {
        Home::SqlTable => {
            let db = booted
                .injector
                .resolve::<dyn holon::di::DbHandleProvider>()
                .handle();
            db.execute_values(
                "INSERT OR REPLACE INTO type_declaration (name, definition) VALUES (?, ?)",
                vec![
                    Value::String(key.to_string()),
                    Value::String(corrupt_text(corruption)),
                ],
            )
            .await
            .expect("insert an unreadable type_declaration row");
        }
        Home::LoroMap => {
            global_doc(booted)
                .await
                .with_write(holon_loro::write_origin::WriteOrigin::BlockOps, |txn| {
                    let map = txn.get_map(TYPE_DECLARATIONS_ROOT);
                    match corruption {
                        Corruption::NonString => map.insert(key, 7i64)?,
                        Corruption::ForeignRoot { kind, emptied, .. } => {
                            kind.plant(txn.doc(), TYPE_DECLARATIONS_ROOT)?;
                            if emptied {
                                kind.empty(txn.doc(), TYPE_DECLARATIONS_ROOT)?;
                            }
                        }
                        other => map.insert(key, corrupt_text(other))?,
                    };
                    Ok(())
                })
                .expect("write an unreadable type-declaration entry");
        }
    }
}

async fn global_doc(booted: &Booted) -> Arc<holon_loro::loro_document::LoroDocument> {
    booted
        .injector
        .try_resolve::<holon_loro::LoroDocumentStore>()
        .expect("a CRDT session registers a LoroDocumentStore")
        .get_doc(holon_loro::loro_document_store::DocScope::Global)
        .await
        .expect("the Global doc")
}

fn foreign_roots(case: &Case) -> impl Iterator<Item = (usize, Corruption)> + '_ {
    case.unreadable
        .iter()
        .enumerate()
        .filter(|(_, (corruption, _))| matches!(corruption, Corruption::ForeignRoot { .. }))
        .map(|(i, (corruption, _))| (i, *corruption))
}

async fn plant_foreign_roots(booted: &Booted, case: &Case, first: bool) {
    for (i, corruption) in foreign_roots(case) {
        if matches!(corruption, Corruption::ForeignRoot { first: f, .. } if f == first) {
            plant(booted, &bad_name(i), corruption, Home::LoroMap).await;
        }
    }
}

async fn foreign_root_values(
    booted: &Booted,
    case: &Case,
) -> Vec<(ForeignRootKind, ForeignRootValue)> {
    if foreign_roots(case).next().is_none() {
        return Vec::new();
    }
    let doc = global_doc(booted).await;
    foreign_roots(case)
        .map(|(_, corruption)| {
            let Corruption::ForeignRoot { kind, .. } = corruption else {
                unreachable!("foreign_roots yields foreign roots")
            };
            let value = doc
                .with_read(|doc| Ok(kind.value(doc, TYPE_DECLARATIONS_ROOT)))
                .expect("read the foreign root");
            (kind, value)
        })
        .collect()
}

async fn check_case(case: &Case) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let planted;
    {
        let first = boot(dir.path(), case.crdt).await;
        plant_foreign_roots(&first, case, true).await;
        for i in 0..case.valid {
            let def = named_type(&valid_name(i));
            dispatch(
                &first,
                holon_app::type_admission::TYPE_ENTITY,
                holon_app::type_admission::DECLARE_TYPE_OP,
                params(&[(
                    "definition",
                    &serde_json::to_string(&def).expect("a TypeDefinition serializes"),
                )]),
            )
            .await
            .expect("declare a valid type");
            dispatch(
                &first,
                &valid_name(i),
                "create",
                params(&[("id", &row_id(i, 1)), ("title", "one")]),
            )
            .await
            .expect("create a row");
        }
        for (i, (corruption, home)) in case.unreadable.iter().enumerate() {
            if !matches!(corruption, Corruption::ForeignRoot { .. }) {
                plant(&first, &bad_name(i), *corruption, *home).await;
            }
        }
        for i in 0..case.seat_refused {
            authority(&first)
                .await
                .store_definition(&seat_refused_type(i))
                .await
                .expect("store a definition the seat refuses");
        }
        plant_foreign_roots(&first, case, false).await;
        planted = foreign_root_values(&first, case).await;
        holon_app::shutdown_session(&first.injector)
            .await
            .expect("shut the first session down");
    }

    let mut reports = Vec::new();
    for round in 0..2usize {
        let booted = boot(dir.path(), case.crdt).await;
        let failed = booted.session.boot_report().failed();
        let refusals = case.unreadable.len() + case.seat_refused;
        if refusals == 0 {
            assert!(failed.is_empty(), "nothing is unreadable: {failed:?}");
        } else {
            assert_eq!(
                failed.iter().map(|(step, _)| *step).collect::<Vec<_>>(),
                vec![BootStep::RedeclareStoredTypes],
                "only the redeclaration step fails: {failed:?}"
            );
            let why = &failed[0].1;
            assert!(
                why.starts_with(&format!("{refusals} stored type declaration(s)")),
                "the count is the number of refused entries of both kinds: {why}"
            );
            let keys: Vec<&str> = why
                .lines()
                .filter_map(|l| l.strip_prefix("  - '")?.split("':").next())
                .collect();
            assert_eq!(keys.len(), refusals, "one line per refused entry: {why}");
            assert!(
                keys.windows(2).all(|w| w[0] < w[1]),
                "the lines are in key order, each key once: {why}"
            );
            for i in 0..case.seat_refused {
                let line = why
                    .lines()
                    .find(|l| l.contains(&format!("'{}':", seat_name(i))))
                    .unwrap_or_else(|| panic!("{} is named: {why}", seat_name(i)));
                assert!(
                    line.contains("holon-nativ"),
                    "the line carries the seat's refusal: {line}"
                );
            }
            for (i, (corruption, _)) in case.unreadable.iter().enumerate() {
                let key = match corruption {
                    Corruption::ForeignRoot { kind, .. } => {
                        kind.root_id(TYPE_DECLARATIONS_ROOT).to_string()
                    }
                    _ => bad_name(i),
                };
                let line = why
                    .lines()
                    .find(|l| l.contains(&format!("'{key}':")))
                    .unwrap_or_else(|| panic!("{key} is named: {why}"));
                let expected = match corruption {
                    Corruption::NonString => "not a JSON string",
                    Corruption::ForeignRoot { .. } => "unread",
                    _ => "not a TypeDefinition",
                };
                assert!(
                    line.contains(expected),
                    "the line carries the error: {line}"
                );
            }
            assert!(
                (0..case.valid).all(|i| !why.contains(&valid_name(i))),
                "no readable type is named: {why}"
            );
            reports.push(why.clone());
        }
        assert_eq!(
            foreign_root_values(&booted, case).await,
            planted,
            "the boot leaves the foreign root as the peer wrote it"
        );
        for i in 0..case.valid {
            let name = valid_name(i);
            dispatch(
                &booted,
                &name,
                "set_field",
                params(&[("id", &row_id(i, 1)), ("field", "title"), ("value", "uno")]),
            )
            .await
            .expect("a readable type takes writes on its stored row");
            dispatch(
                &booted,
                &name,
                "create",
                params(&[("id", &row_id(i, 2 + round)), ("title", "two")]),
            )
            .await
            .expect("a readable type takes new rows");
        }
        let refused_keys = (0..case.unreadable.len())
            .map(bad_name)
            .chain((0..case.seat_refused).map(seat_name));
        for key in refused_keys {
            assert!(
                dispatch(&booted, &key, "create", params(&[("id", "x:1")]))
                    .await
                    .is_err(),
                "a refused entry takes no writes"
            );
        }
        holon_app::shutdown_session(&booted.injector)
            .await
            .expect("shut the session down");
    }
    if let [a, b] = reports.as_slice() {
        assert_eq!(
            a, b,
            "the unreadable entries stay stored: the next boot reports the same"
        );
    }
}

fn case_strategy() -> impl Strategy<Value = Case> {
    let corruption = prop_oneof![
        Just(Corruption::InvalidJson),
        Just(Corruption::OldShape),
        Just(Corruption::Empty),
        Just(Corruption::NonString),
        (foreign_root_kind(), any::<bool>(), any::<bool>()).prop_map(|(kind, emptied, first)| {
            Corruption::ForeignRoot {
                kind,
                emptied,
                first,
            }
        }),
    ];
    (
        any::<bool>(),
        // Zero leaves the type declarations map empty or absent.
        0usize..=4,
        proptest::collection::vec((corruption, any::<bool>()), 0..=3),
        0usize..=2,
    )
        .prop_map(|(crdt, valid, rows, seat_refused)| {
            let mut kinds_drawn = Vec::new();
            Case {
                crdt,
                valid,
                seat_refused,
                unreadable: rows
                    .into_iter()
                    // A doc holds one root per name and kind.
                    .filter(|(corruption, _)| match corruption {
                        Corruption::ForeignRoot { kind, .. } if kinds_drawn.contains(kind) => false,
                        Corruption::ForeignRoot { kind, .. } => {
                            kinds_drawn.push(*kind);
                            true
                        }
                        _ => true,
                    })
                    .map(|(corruption, in_loro)| {
                        // A Loro map exists only under the CRDT authority, and
                        // a non-string value or a foreign root only fits a doc.
                        let doc_only = matches!(
                            corruption,
                            Corruption::NonString | Corruption::ForeignRoot { .. }
                        );
                        let home = if crdt && (in_loro || doc_only) {
                            Home::LoroMap
                        } else {
                            Home::SqlTable
                        };
                        let corruption = if home == Home::SqlTable && doc_only {
                            Corruption::InvalidJson
                        } else {
                            corruption
                        };
                        (corruption, home)
                    })
                    .collect(),
            }
        })
}

fn run(case: &Case) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build a runtime")
        .block_on(check_case(case));
}

fn fixed(crdt: bool, valid: usize, unreadable: &[(Corruption, Home)], seat_refused: usize) -> Case {
    Case {
        crdt,
        valid,
        unreadable: unreadable.to_vec(),
        seat_refused,
    }
}

fn fixed_examples() -> [Case; 8] {
    [
        fixed(false, 1, &[(Corruption::OldShape, Home::SqlTable)], 0),
        fixed(true, 2, &[(Corruption::OldShape, Home::SqlTable)], 0),
        fixed(true, 1, &[(Corruption::NonString, Home::LoroMap)], 0),
        fixed(
            true,
            2,
            &[
                (Corruption::InvalidJson, Home::LoroMap),
                (Corruption::Empty, Home::SqlTable),
            ],
            0,
        ),
        fixed(
            false,
            1,
            &[
                (Corruption::InvalidJson, Home::SqlTable),
                (Corruption::Empty, Home::SqlTable),
            ],
            2,
        ),
        fixed(
            true,
            2,
            &[(
                Corruption::ForeignRoot {
                    kind: ForeignRootKind::List,
                    emptied: false,
                    first: false,
                },
                Home::LoroMap,
            )],
            0,
        ),
        fixed(
            true,
            0,
            &[(
                Corruption::ForeignRoot {
                    kind: ForeignRootKind::Counter,
                    emptied: false,
                    first: false,
                },
                Home::LoroMap,
            )],
            0,
        ),
        fixed(
            true,
            2,
            &[
                (
                    Corruption::ForeignRoot {
                        kind: ForeignRootKind::List,
                        emptied: false,
                        first: true,
                    },
                    Home::LoroMap,
                ),
                (
                    Corruption::ForeignRoot {
                        kind: ForeignRootKind::Text,
                        emptied: true,
                        first: false,
                    },
                    Home::LoroMap,
                ),
            ],
            0,
        ),
    ]
}

/// Each fixed example also runs alone, so the first red does not hide the
/// others; the property below runs all of them first, then random cases.
#[test]
fn example_old_shape_row_in_sql() {
    run(&fixed_examples()[0]);
}

#[test]
fn example_old_shape_row_with_a_loro_authority() {
    run(&fixed_examples()[1]);
}

#[test]
fn example_loro_non_string_value() {
    run(&fixed_examples()[2]);
}

#[test]
fn example_two_unreadable_entries_in_two_homes() {
    run(&fixed_examples()[3]);
}

#[test]
fn example_unreadable_and_seat_refused_entries_merge_in_key_order() {
    run(&fixed_examples()[4]);
}

#[test]
fn example_list_root_under_the_type_declarations_name() {
    run(&fixed_examples()[5]);
}

/// A counter root is never empty to name resolution, and no map root exists.
#[test]
fn example_counter_root_and_no_type_declarations_map() {
    run(&fixed_examples()[6]);
}

/// The map wins the name over both roots: one written before the map's
/// entries, one emptied after them.
#[test]
fn example_roots_the_type_declarations_map_hides() {
    run(&fixed_examples()[7]);
}

/// The SQL `definition` column is NOT NULL, so a row without a definition
/// cannot be stored; its Loro analogue is the non-string value below.
#[test]
fn one_unreadable_stored_type_is_refused_alone() {
    // Fixed cases run through the same property before the random ones.
    for case in fixed_examples() {
        run(&case);
    }
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 6,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    runner
        .run(&case_strategy(), |case| {
            run(&case);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_declared_type_takes_writes_after_a_restart_and_a_refused_one_is_disclosed() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    rt.block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let kept = kept_type();
        let mut refused = TypeDefinition::new(
            "gen_refused",
            [FieldSchema::new("id", "TEXT").primary_key()]
                .into_iter()
                .chain(FieldSchema::overflow_pair())
                .collect(),
        );
        refused.home =
            Some(HomeProfileId::parse("holon-nativ").expect("well-formed, merely wrong"));
        {
            let first = boot(dir.path(), true).await;
            dispatch(
                &first,
                holon_app::type_admission::TYPE_ENTITY,
                holon_app::type_admission::DECLARE_TYPE_OP,
                params(&[(
                    "definition",
                    &serde_json::to_string(&kept).expect("a TypeDefinition serializes"),
                )]),
            )
            .await
            .expect("declare gen_kept");
            dispatch(
                &first,
                "gen_kept",
                "create",
                params(&[("id", "gen-kept:k1"), ("title", "one")]),
            )
            .await
            .expect("create a gen_kept row");
            // Past the seat, as a definition stored by a build whose profiles
            // admitted it would be.
            authority(&first)
                .await
                .store_definition(&refused)
                .await
                .expect("store a definition the seat refuses");
            holon_app::shutdown_session(&first.injector)
                .await
                .expect("shut the first session down");
        }

        let second = boot(dir.path(), true).await;
        let failed = second.session.boot_report().failed();
        assert_eq!(
            failed.iter().map(|(step, _)| *step).collect::<Vec<_>>(),
            vec![BootStep::RedeclareStoredTypes],
            "only the redeclaration step fails: {failed:?}"
        );
        let why = &failed[0].1;
        assert!(
            why.contains("gen_refused") && why.contains("holon-nativ") && !why.contains("gen_kept"),
            "the failure names the refused type and its refusal, and only it: {why}"
        );

        dispatch(
            &second,
            "gen_kept",
            "set_field",
            params(&[("id", "gen-kept:k1"), ("field", "title"), ("value", "uno")]),
        )
        .await
        .expect("the redeclared type takes writes on a stored row");
        dispatch(
            &second,
            "gen_kept",
            "create",
            params(&[("id", "gen-kept:k2"), ("title", "two")]),
        )
        .await
        .expect("the redeclared type takes new rows");
        let mut titles: Vec<String> = authority(&second)
            .await
            .read_all(&kept)
            .await
            .expect("read gen_kept")
            .iter()
            .map(|row| {
                row.get("title")
                    .and_then(|v| v.as_string())
                    .expect("a title")
                    .to_string()
            })
            .collect();
        titles.sort();
        assert_eq!(titles, vec!["two".to_string(), "uno".to_string()]);
        assert!(
            dispatch(
                &second,
                "gen_refused",
                "create",
                params(&[("id", "gen-refused:r1")])
            )
            .await
            .is_err(),
            "a refused type takes no writes"
        );
        holon_app::shutdown_session(&second.injector)
            .await
            .expect("shut the second session down");
    });
}
