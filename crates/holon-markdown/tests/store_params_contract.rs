//! What an ingest hands the store, over enumerated documents and blocks:
//! every field `build_block_params` emits is gated by `content_differs`, a
//! file never declares one id twice, and every authored property key either
//! reaches the store or refuses the file by name.

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::Priority;
use holon_api::StateCategory;
use holon_api::StorageEntity;
use holon_api::TaskState;
use holon_api::Timestamp;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::inline_mark::InlineMark;
use holon_api::inline_mark::MarkSpan;
use holon_core::file_format::FileFormatAdapter;
use holon_markdown::LogseqMarkdownAdapter;
use holon_markdown::build::FOREIGN_OPAQUE_KEY;
use holon_markdown::build::opaque_block;
use holon_markdown::params::build_block_params;
use holon_markdown::params::content_differs;
use holon_org_format::OrgBlockExt;

use crate::documents::Flavor;
use crate::documents::every_doc;

fn doc_uri() -> EntityUri {
    EntityUri::file("P.md")
}

/// The params of a create, without the clock fields no edit of the file moves.
fn params_without_clock(block: &Block) -> StorageEntity {
    let mut params = build_block_params(block, &block.parent_id, &doc_uri(), None);
    params.remove("created_at");
    params.remove("updated_at");
    params
}

fn base() -> Block {
    Block::new_text(EntityUri::block("b"), EntityUri::block("p"), "t")
}

/// `base` with exactly one emitted field changed, for each emitted field and
/// each of two values where the field has more than one.
fn one_field_variants() -> Vec<Block> {
    let mut out: Vec<Block> = vec![base()];
    let mut with = |f: &dyn Fn(&mut Block)| {
        let mut b = base();
        f(&mut b);
        out.push(b);
    };
    with(&|b| b.parent_id = EntityUri::block("q"));
    with(&|b| b.content = "u".into());
    with(&|b| b.content_type = ContentType::Image);
    with(&|b| b.content_type = ContentType::Source);
    for lang in ["python", "rust"] {
        with(&|b| {
            b.content_type = ContentType::Source;
            b.source_language = Some(lang.parse().unwrap());
        });
        // Not emitted for a text block: must NOT count as a difference.
        with(&|b| b.source_language = Some(lang.parse().unwrap()));
    }
    with(&|b| b.marks = Some(vec![MarkSpan::new(0, 1, InlineMark::Bold)]));
    with(&|b| b.marks = Some(vec![MarkSpan::new(0, 1, InlineMark::Italic)]));
    for state in ["TODO", "DONE"] {
        with(&|b| b.set_task_state(Some(TaskState::new(state, StateCategory::Active))));
    }
    for letter in ['A', 'B'] {
        with(&|b| b.set_priority(Some(Priority::from_letter(letter).unwrap())));
    }
    for day in ["<2026-01-01 Thu>", "<2026-02-02 Mon>"] {
        with(&|b| b.set_scheduled(Some(Timestamp::parse(day).unwrap())));
        with(&|b| b.set_deadline(Some(Timestamp::parse(day).unwrap())));
    }
    with(&|b| {
        b.tags.insert("t".to_string());
    });
    with(&|b| b.requires.push(EntityUri::block("r")));
    for v in ["x", "y"] {
        with(&|b| b.set_property("mood", v.to_string()));
    }
    for kind in ["query", "callout"] {
        with(&|b| b.set_property(FOREIGN_OPAQUE_KEY, kind.to_string()));
    }
    out
}

#[test]
fn content_differs_is_exactly_whether_the_emitted_params_differ() {
    let variants = one_field_variants();
    for a in &variants {
        for b in &variants {
            assert_eq!(
                content_differs(a, b),
                params_without_clock(a) != params_without_clock(b),
                "gate and params disagree for\n a = {a:?}\n b = {b:?}"
            );
        }
    }
}

fn logseq_params(source: &str, previous: Option<&Block>) -> (Vec<Block>, Vec<StorageEntity>) {
    let adapter = LogseqMarkdownAdapter::new();
    let parsed = Flavor::Logseq.parse(source).unwrap();
    let params = parsed
        .blocks
        .iter()
        .map(|b| adapter.build_block_params(b, &b.parent_id, &parsed.document.id, previous))
        .collect();
    (parsed.blocks, params)
}

#[test]
fn a_moved_declared_block_is_an_update() {
    let l = LogseqMarkdownAdapter::new();
    let before = Flavor::Logseq.parse("- A\n- B\n  id:: kid\n").unwrap();
    let after = Flavor::Logseq.parse("- A\n  - B\n    id:: kid\n").unwrap();
    assert_ne!(before.blocks[1].parent_id, after.blocks[1].parent_id);
    assert!(l.content_differs(&before.blocks[1], &after.blocks[1]));
}

#[test]
fn an_opaque_block_carries_its_kind_into_the_params() {
    let (blocks, params) = logseq_params("- {{query (todo TODO)}}\n", None);
    assert_eq!(
        blocks[0].get_property_str(FOREIGN_OPAQUE_KEY).as_deref(),
        Some("query")
    );
    assert_eq!(
        params[0].get(FOREIGN_OPAQUE_KEY),
        Some(&Value::String("query".into()))
    );
}

#[test]
fn every_parsed_block_forwards_its_disclosure_and_clears_a_dropped_one() {
    for flavor in Flavor::ALL {
        let adapter = flavor.adapter();
        for doc in every_doc(flavor, 2) {
            let parsed = match flavor.parse(&doc.source) {
                Ok(parsed) => parsed,
                Err(_) => continue,
            };
            let previous = opaque_block(
                EntityUri::block("prev"),
                EntityUri::block("p"),
                "query",
                "{{query}}",
            );
            for b in &parsed.blocks {
                let create = adapter.build_block_params(b, &b.parent_id, &parsed.document.id, None);
                let update = adapter.build_block_params(
                    b,
                    &b.parent_id,
                    &parsed.document.id,
                    Some(&previous),
                );
                match b.get_property_str(FOREIGN_OPAQUE_KEY) {
                    Some(kind) => {
                        assert_eq!(create.get(FOREIGN_OPAQUE_KEY), Some(&Value::String(kind)));
                    }
                    None => {
                        assert!(!create.contains_key(FOREIGN_OPAQUE_KEY), "{:?}", doc.source);
                        assert_eq!(
                            update.get(FOREIGN_OPAQUE_KEY),
                            Some(&Value::REMOVED),
                            "{:?}",
                            doc.source
                        );
                    }
                }
            }
        }
    }
}

const REFUSED_DUPLICATES: &[(Flavor, &str)] = &[
    (Flavor::Logseq, "- a\n  id:: same\n- b\n  id:: same\n"),
    (Flavor::Obsidian, "a ^x\n\nb ^x\n"),
];

#[test]
fn a_file_is_refused_exactly_when_it_declares_an_id_twice() {
    for (flavor, source) in REFUSED_DUPLICATES {
        assert!(flavor.parse(source).is_err(), "{source:?}");
    }
    for flavor in Flavor::ALL {
        for doc in every_doc(flavor, 3) {
            let result = flavor.parse(&doc.source);
            match doc.first_duplicate() {
                None => {
                    result.unwrap_or_else(|e| panic!("{:?} refused: {e:#}", doc.source));
                }
                Some((id, first, second)) => {
                    let message = format!(
                        "{:#}",
                        result.err().unwrap_or_else(|| {
                            panic!("{:?} declares {id:?} twice and was accepted", doc.source)
                        })
                    );
                    for needle in [
                        format!("{id:?}"),
                        format!("line {first} "),
                        format!("line {second}"),
                    ] {
                        assert!(message.contains(&needle), "{needle} not in {message}");
                    }
                }
            }
        }
    }
}

/// Authored keys a file may carry: plain names reach the store.
const STORED: &[&str] = &["mood", "type"];

/// Keys an adapter reads from the file itself, in either case: the block id
/// and the fold state.
const CONSUMED: &[&str] = &["id", "collapsed", "ID", "COLLAPSED"];

/// Planning keys a LogSeq block reads as its own schedule or deadline.
const PLANNING_ON_LOGSEQ_BLOCK: &[&str] = &["SCHEDULED", "DEADLINE"];

/// The org hidden-key list, named here because `is_hidden_drawer_key` keeps it
/// private; `refused_keys` asserts each entry still satisfies that predicate.
const HIDDEN: &[&str] = &[
    "level",
    "sequence",
    "task_state",
    "task_state_category",
    "priority",
    "tags",
    "requires",
    "advice_suppressed",
    "contributes_to",
    "scheduled",
    "deadline",
    "org_properties",
    "TODO",
    "PRIORITY",
    "TAGS",
    "SCHEDULED",
    "DEADLINE",
    "ID",
    "COLLAPSED",
    "WIDGET_ONLY",
    "_source_header_args",
    "_source_results",
];

/// Every key a file must not carry: the hidden keys, a `_` carrier and every
/// column of the block schema, less the consumed keys and the edge keys the
/// parser lifts into typed edges.
fn refused_keys() -> Vec<&'static str> {
    for k in HIDDEN.iter().chain(&["_x"]) {
        assert!(
            holon_org_format::models::is_hidden_drawer_key(k),
            "{k} is not hidden"
        );
    }
    HIDDEN
        .iter()
        .copied()
        .chain(["_x"])
        .chain(holon_api::schema::BLOCK.columns())
        .filter(|k| !CONSUMED.contains(k) && !holon_api::EdgeField::is_edge_drawer_key(k))
        .collect()
}

fn authored(flavor: Flavor, key: &str, page_level: bool) -> String {
    match (flavor, page_level) {
        (Flavor::Logseq, false) => format!("- a\n  {key}:: v\n"),
        (Flavor::Logseq, true) => format!("{key}:: v\n\n- a\n"),
        (Flavor::Obsidian, true) => format!("---\n{key}: v\n---\nbody\n"),
        (Flavor::Obsidian, false) => unreachable!("Obsidian blocks carry no properties"),
    }
}

#[test]
fn an_authored_property_key_reaches_the_store_or_refuses_the_file_by_name() {
    let cases = [
        (Flavor::Logseq, false),
        (Flavor::Logseq, true),
        (Flavor::Obsidian, true),
    ];
    for (flavor, page_level) in cases {
        let keys = STORED
            .iter()
            .map(|k| (*k, false))
            .chain(refused_keys().into_iter().map(|k| (k, true)));
        for (key, refused) in keys {
            if flavor == Flavor::Logseq && !page_level && PLANNING_ON_LOGSEQ_BLOCK.contains(&key) {
                continue;
            }
            let source = authored(flavor, key, page_level);
            match flavor.parse(&source) {
                Err(e) => {
                    assert!(refused, "{source:?} refused: {e:#}");
                    let message = format!("{e:#}");
                    assert!(message.ends_with(&format!(
                        ": the property `{key}` names a storage column or an internal key and cannot be stored as a property; rename the property"
                    )), "{message}");
                }
                Ok(parsed) => {
                    assert!(
                        !refused,
                        "{source:?} accepted: the key is dropped on the way"
                    );
                    let holder = if page_level {
                        &parsed.document
                    } else {
                        &parsed.blocks[0]
                    };
                    assert_eq!(
                        holder.get_property_str(key).as_deref(),
                        Some("v"),
                        "{source:?}"
                    );
                    for b in &parsed.blocks {
                        let params = flavor.adapter().build_block_params(
                            b,
                            &b.parent_id,
                            &parsed.document.id,
                            None,
                        );
                        for (k, v) in b.drawer_properties() {
                            if !holon_api::EdgeField::is_edge_drawer_key(&k) {
                                assert_eq!(
                                    params.get(k.as_str()),
                                    Some(&Value::String(v)),
                                    "{source:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_column_named_logseq_property_refuses_the_file_with_its_line() {
    let message = format!(
        "{:#}",
        Flavor::Logseq
            .parse("- a\n- b\n  content:: high\n")
            .err()
            .expect("the parse is refused")
    );
    assert!(message.contains("line 2"), "{message}");
    assert!(message.contains("`content`"), "{message}");
}

#[test]
fn the_block_id_and_fold_state_keys_are_consumed_not_stored_in_either_case_on_a_block() {
    for key in CONSUMED {
        let source = format!("- a\n  {key}:: v\n");
        let parsed = Flavor::Logseq
            .parse(&source)
            .unwrap_or_else(|e| panic!("{source:?} refused: {e:#}"));
        for block in &parsed.blocks {
            assert_eq!(block.get_property_str(key), None, "{source:?}");
        }
    }
}

/// Assures what the enumerated key table cannot: over drawn keys, a LogSeq
/// block's `parse` refuses a key exactly when the org predicate hides it or
/// the schema has a column of that name, so a key the predicate gains is
/// caught as soon as the draw reaches it (the named list is only the seed).
#[test]
fn a_logseq_block_refuses_a_key_iff_it_is_hidden_or_a_column() {
    let mut keys: Vec<String> = HIDDEN.iter().map(|k| k.to_string()).collect();
    keys.extend(HIDDEN.iter().map(|k| k.to_uppercase()));
    keys.extend(HIDDEN.iter().map(|k| k.to_lowercase()));
    keys.extend(STORED.iter().map(|k| k.to_string()));
    keys.extend(
        holon_api::schema::BLOCK
            .columns()
            .into_iter()
            .map(|k| k.to_string()),
    );
    keys.extend(["_x", "_", "_Zz9", "mood2", "Mood", "foo_bar", "x"].map(String::from));
    for key in keys {
        let consumed = key.eq_ignore_ascii_case("id")
            || key.eq_ignore_ascii_case("collapsed")
            || PLANNING_ON_LOGSEQ_BLOCK.contains(&key.as_str())
            || holon_api::EdgeField::is_edge_drawer_key(&key);
        if consumed {
            continue;
        }
        let expect_refused = holon_org_format::models::is_hidden_drawer_key(&key)
            || holon_api::schema::is_block_column(&key);
        let source = authored(Flavor::Logseq, &key, false);
        assert_eq!(
            Flavor::Logseq.parse(&source).is_err(),
            expect_refused,
            "{source:?}"
        );
    }
}
