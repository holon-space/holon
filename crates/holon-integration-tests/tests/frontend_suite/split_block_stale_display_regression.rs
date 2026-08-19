//! **Deterministic regression for the `inv-displayed-text/viewmodel`
//! stale-split-display bug** (cycle-4 land-gate RED, keystone-found).
//!
//! The composed keystone (`general_e2e_composed_pbt`) shrank to a case where,
//! after a sequence of `SplitBlock` ops, two blocks whose LAST content-changing
//! op was a split kept showing their PRE-split content at the ViewModel layer:
//!
//! ```text
//!   rendered_text@block=block:bulk-5-6  shown "ip0JJ2Jn 9Yo uB81eT"  expected "ip0JJ2Jn 9Yo u"
//!   rendered_text@block=block:bulk-5-7  shown "xHdKRDid  D"          expected "xHdK"
//! ```
//!
//! The red run never persisted a proptest seed, so this test replays the SHRUNK
//! minimal failing input DIRECTLY (constructed `ReferenceState` via the exact
//! keystone boot `wide_e2e_ref()` for the reproducing wiring
//! `{Loro, Org, Turso} / {ActionEngine}` == `full_headless`, plus the recorded
//! transition list) through the SAME composed runner + full invariant catalog
//! the keystone uses. It is the permanent gate for the fix: RED before, GREEN
//! after, and it fails LOUD on any future regression of the same class.
//!
//! @pbt kind regression
//! @pbt covers split-block-stale-viewmodel-display — origin content truncation
//! must reach the reactive row cache the ViewModel renders from

use holon_api::EdgeFieldUpdate;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref_for;
use holon_integration_tests::pbt::transitions::BulkExternalAdd;
use holon_integration_tests::pbt::transitions::CreateBlockUnderFocus;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::Indent;
use holon_integration_tests::pbt::transitions::Outdent;
use holon_integration_tests::pbt::transitions::SetEdgeField;
use holon_integration_tests::pbt::transitions::SplitBlock;
use holon_pbt_core::Actor;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::SyncAdapter;
use holon_pbt_core::Wiring;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

fn b(raw: &str) -> EntityUri {
    EntityUri::parse(raw).expect("valid block uri")
}

fn split(id: &str, position: usize) -> E2ETransition {
    E2ETransition::SplitBlock(SplitBlock {
        block_id: b(id),
        position,
    })
}

fn create(content: &str, id: Option<&str>) -> E2ETransition {
    E2ETransition::CreateBlockUnderFocus(CreateBlockUnderFocus {
        content: content.to_string(),
        id: id.map(b),
    })
}

fn indent(id: &str) -> E2ETransition {
    E2ETransition::Indent(Indent { block_id: b(id) })
}

fn outdent(id: &str) -> E2ETransition {
    E2ETransition::Outdent(Outdent { block_id: b(id) })
}

#[test]
fn split_block_stale_viewmodel_display_regression() {
    // The shrunk minimal failing input's `BulkExternalAdd` payload: 10 text
    // blocks under `block:structural-page` with the exact parents/contents the
    // keystone drew (bulk-5-6 / bulk-5-7 are the ones that went stale).
    let bulk_blocks: Vec<Block> = [
        ("bulk-5-0", "structural-page", "w iLtsBpxL 7gim"),
        ("bulk-5-1", "bulk-5-0", "x"),
        ("bulk-5-2", "structural-page", "m€ßx中 ññz ñ"),
        ("bulk-5-3", "structural-page", "#+ V ox"),
        ("bulk-5-4", "structural-page", "thUhjpAw4m"),
        ("bulk-5-5", "structural-page", "H7P Bi "),
        ("bulk-5-6", "bulk-5-5", "ip0JJ2Jn 9Yo uB81eT"),
        ("bulk-5-7", "bulk-5-5", "xHdKRDid  D"),
        ("bulk-5-8", "bulk-5-3", "日€t日😀€"),
        ("bulk-5-9", "bulk-5-3", "#+ R"),
    ]
    .into_iter()
    .map(|(id, parent, content)| {
        Block::new_text(
            b(&format!("block:{id}")),
            b(&format!("block:{parent}")),
            content,
        )
    })
    .collect();

    let transitions: Vec<E2ETransition> = vec![
        split("block:parent", 1),
        create("rot", Some("block:gen-1")),
        split("block:c2", 1),
        split("block::split-0", 5),
        create("o", Some("block:gen-4")),
        E2ETransition::BulkExternalAdd(BulkExternalAdd {
            doc_uri: b("block:structural-page"),
            blocks: bulk_blocks,
        }),
        split("block:bulk-5-1", 1),
        split("block:bulk-5-0", 11),
        split("block:bulk-5-0", 7),
        split("block:bulk-5-6", 19),
        create("hhvifhs", None),
        indent("block:bulk-5-4"),
        split("block:bulk-5-1", 0),
        split("block:bulk-5-2", 13),
        split("block:bulk-5-6", 14),
        outdent("block::split-22"),
        split("block::split-21", 3),
        split("block:bulk-5-7", 6),
        E2ETransition::SetEdgeField(SetEdgeField {
            block_id: b("block::split-24"),
            update: EdgeFieldUpdate::Requires(vec![b("block:bulk-5-8")]),
        }),
        outdent("block:bulk-5-8"),
        split("block:bulk-5-7", 4),
        split("block::create-19", 4),
        outdent("block:bulk-5-7"),
        indent("block::split-17"),
        split("block:bulk-5-9", 2),
        split("block:bulk-5-5", 2),
    ];

    // The EXACT shrunk reproducing wiring the keystone landed on: storage
    // {Loro, Org, Turso}, no sync, actors {ActionEngine}. (NOT `full_headless`,
    // which additionally carries Markdown storage, Todoist sync, and the
    // MCPServer actor — a richer component set whose different CDC/settle timing
    // masks the reactive-consumer-drain race this seed exercises.)
    let wiring = Wiring::custom(
        [
            StorageAdapter::Loro,
            StorageAdapter::Org,
            StorageAdapter::Turso,
        ],
        std::iter::empty::<SyncAdapter>(),
        [Actor::ActionEngine],
    );
    let initial_state = wide_e2e_ref_for(&wiring);
    let config = Config {
        cases: 1,
        ..Config::default()
    };
    <ComposedSut<WideE2E> as StateMachineTest>::test_sequential(
        config,
        initial_state,
        transitions,
        None,
    );
}
