//! A live `holon_sql` query with a `WITH RECURSIVE` CTE renders every row the
//! CTE reaches, not only the anchor rows, and keeps doing so as the tree
//! changes.
//!
//! Entry `2026-10-07-recursive-live-query-renders-only-roots`. The query is
//! the vault's first "Open decisions" form: open decision blocks plus all
//! their descendants.
//!
//! @pbt kind harness
//! @pbt covers recursive-live-query-rows — a recursive live query renders
//! the anchor rows and every descendant the recursive member reaches
//! @pbt slips-if-removed a recursive live query silently renders only its
//! anchor rows and no test sees it

use std::collections::BTreeSet;
use std::time::Duration;
use std::time::Instant;

use holon_api::QueryLanguage;
use holon_integration_tests::pbt::frontend_slice::components::HeadlessFrontendComponent;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::capabilities::SutRenderer;

const DECISIONS_ORG: &str = "#+ID: decisions\n\
* ? Pick a colour :decision:\n:PROPERTIES:\n:ID: dec-colour\n:END:\n\
** Red\n:PROPERTIES:\n:ID: dec-colour-red\n:option: a\n:END:\n\
*** Red is loud\n:PROPERTIES:\n:ID: dec-colour-red-note\n:END:\n\
** Blue\n:PROPERTIES:\n:ID: dec-colour-blue\n:option: b\n:END:\n\
* ? Pick a size :decision:\n:PROPERTIES:\n:ID: dec-size\n:END:\n\
** Large\n:PROPERTIES:\n:ID: dec-size-large\n:option: a\n:END:\n\
* ? Not a decision\n:PROPERTIES:\n:ID: undecided-task\n:END:\n\
** Untagged child\n:PROPERTIES:\n:ID: undecided-child\n:END:\n";

const OPEN_DECISIONS_SQL: &str = "WITH RECURSIVE d(id) AS (
  SELECT b.id FROM block b
  WHERE json_extract(b.properties, '$.task_state') = '?'
    AND EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'decision')
  UNION ALL
  SELECT c.id FROM block c JOIN d ON c.parent_id = d.id
)
SELECT b.*
FROM block b JOIN d ON d.id = b.id
ORDER BY b.sort_key";

const OPEN_DECISIONS_PAGE_ORG: &str = "#+ID: open-decisions-page\n\
* Open decisions\n:PROPERTIES:\n:ID: open-decisions\n:END:\n";

const ANCHORS: [&str; 2] = ["block:dec-colour", "block:dec-size"];
const DESCENDANTS: [&str; 4] = [
    "block:dec-colour-red",
    "block:dec-colour-red-note",
    "block:dec-colour-blue",
    "block:dec-size-large",
];

fn seeded_query_page() -> String {
    format!(
        "{OPEN_DECISIONS_PAGE_ORG}#+BEGIN_SRC holon_sql :id open-decisions::src::0\n\
         {OPEN_DECISIONS_SQL}\n#+END_SRC\n"
    )
}

/// Two open decisions with `options` options each; every option carries two
/// notes and every note one sub-note. Returns the org text and every block id
/// the query must render.
fn wide_decisions(options: usize) -> (String, BTreeSet<String>) {
    let mut org = String::from("#+ID: decisions\n");
    let mut ids = BTreeSet::new();
    let mut block = |org: &mut String, stars: &str, title: &str, id: String, extra: &str| {
        org.push_str(&format!(
            "{stars} {title}\n:PROPERTIES:\n:ID: {id}\n{extra}:END:\n"
        ));
        ids.insert(format!("block:{id}"));
    };
    for d in 0..2 {
        let dec = format!("dec-{d}");
        block(
            &mut org,
            "*",
            &format!("? Decide {d} :decision:"),
            dec.clone(),
            "",
        );
        for o in 0..options {
            let opt = format!("{dec}-o{o}");
            block(
                &mut org,
                "**",
                &format!("Option {o}"),
                opt.clone(),
                &format!(":option: o{o}\n"),
            );
            for n in 0..2 {
                let note = format!("{opt}-n{n}");
                block(&mut org, "***", &format!("pro: note {n}"), note.clone(), "");
                block(&mut org, "****", "detail", format!("{note}-x"), "");
            }
        }
    }
    (org, ids)
}

async fn boot(query_page_org: &str) -> HeadlessFrontendComponent {
    boot_with(DECISIONS_ORG, query_page_org).await
}

async fn boot_with(decisions_org: &str, query_page_org: &str) -> HeadlessFrontendComponent {
    let comp = HeadlessFrontendComponent::new_with_loro(
        &[
            ("decisions.org", decisions_org),
            ("open-decisions.org", query_page_org),
        ],
        Duration::from_millis(500),
        true,
    )
    .await;
    comp.apply_navigate_focus(CapRegion::Main, &EntityUri::block("open-decisions-page"))
        .await;
    comp
}

/// Waits until the rendered decision rows equal `expected`, and panics with
/// the rows it did render when they never do.
async fn assert_renders(comp: &HeadlessFrontendComponent, expected: &BTreeSet<&str>, when: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let got: BTreeSet<String> = comp
            .widget_tree_snapshot()
            .await
            .walk()
            .filter(|n| n.kind == "tree_item")
            .filter_map(|n| n.entity_id.clone())
            .filter(|id| id.starts_with("block:dec-") || id.starts_with("block:undecided"))
            .collect();
        if got.iter().map(String::as_str).eq(expected.iter().copied()) {
            return;
        }
        if Instant::now() >= deadline {
            let head_tree = comp
                .render_tree_of(&EntityUri::parse("block:open-decisions").expect("id"))
                .await;
            panic!(
                "{when}: the recursive live query rendered {} rows {got:?}, expected {} rows \
                 {expected:?} (the anchors AND every descendant). A silent subset of the \
                 recursive result is a fail-loud violation.\n--- render_tree_of(open-decisions): \
                 {head_tree:?}",
                got.len(),
                expected.len(),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn full_set() -> BTreeSet<&'static str> {
    ANCHORS.iter().chain(DESCENDANTS.iter()).copied().collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn seeded_recursive_query_renders_and_follows_descendants() {
    let comp = boot(&seeded_query_page()).await;
    let mut expected = full_set();
    assert_renders(&comp, &expected, "after boot").await;

    comp.create_block("dec-size-large-note", "dec-size-large", "Large is heavy")
        .await;
    comp.create_block(
        "dec-colour-red-note-why",
        "dec-colour-red-note",
        "Because it is red",
    )
    .await;
    expected.insert("block:dec-size-large-note");
    expected.insert("block:dec-colour-red-note-why");
    assert_renders(&comp, &expected, "after adding a child and a grandchild").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn recursive_query_created_in_a_running_app_renders_descendants() {
    let comp = boot(OPEN_DECISIONS_PAGE_ORG).await;
    comp.create_source_block(
        "open-decisions::src::0",
        "open-decisions",
        QueryLanguage::HolonSql,
        OPEN_DECISIONS_SQL,
    )
    .await;
    assert_renders(&comp, &full_set(), "after creating the query block").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn recursive_query_moved_into_place_renders_descendants() {
    let comp = boot(OPEN_DECISIONS_PAGE_ORG).await;
    comp.create_source_block(
        "open-decisions::src::0",
        "open-decisions-page",
        QueryLanguage::HolonSql,
        OPEN_DECISIONS_SQL,
    )
    .await;
    comp.move_block("open-decisions::src::0", "open-decisions")
        .await;
    assert_renders(
        &comp,
        &full_set(),
        "after moving the query block under its heading",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn recursive_query_renders_a_vault_sized_decision_set() {
    let (org, ids) = wide_decisions(4);
    let comp = boot_with(&org, &seeded_query_page()).await;
    let expected: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    assert_renders(&comp, &expected, "after boot").await;
}
