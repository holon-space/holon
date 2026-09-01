//! A `.cook` file in the vault reaches `CookFormatAdapter` on a REAL boot, and
//! is never written back.
//!
//! Kitchen Inc A landed the adapter but nothing routed a vault file to it: the
//! controller held ONE adapter and the watcher's extension filter was
//! hardcoded to `.org`, so every `.cook` file was scanned and then dropped.
//! Inc A2's `FormatRegistry` is what makes "cooklang files are authoritative"
//! (K1) true of a real vault rather than of a crate-level fixture.
//!
//! Two properties, and the second is the one that bites: routing a read-only
//! format INTO the controller also exposes it to the controller's write half.
//! A recipe's identity is name-chain-derived (cooklang embeds no id), and the
//! page-file path derivation appends `.org` unconditionally
//! (`VaultPath::page_file_from_name_chain`), so an ungated write-back would
//! not overwrite the recipe — it would mint a SECOND, org-format home beside
//! it and quietly diverge from the authoritative file.
//!
//! @pbt kind harness
//! @pbt covers cook-vault-ingest — a `.cook` file in a real vault ingests as a
//! document with its steps, and its write-back is refused visibly with no
//! second home minted
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone drives an
//! org-only vault and has no second-format file in its fixture

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::QueryLanguage;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_kitchen::COOK_BLOCKERS_SQL;
use holon_kitchen::COOKABLE_RECIPES_SQL;
use holon_kitchen::CookBlockReason;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_test_writer()
        .try_init();
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

/// The content of the vault file named `name`, or `None` when the vault holds
/// no such file. Goes through the `FileSystem` port and matches on file NAME,
/// so it is immune to how the in-memory tree spells its root.
async fn read_vault_file(
    env: &holon_integration_tests::TestEnvironment,
    name: &str,
) -> Option<String> {
    use holon_filesystem::FileSystem;
    let scanned = env
        .org_fs
        .scan_directory(env.org_root())
        .await
        .expect("scan the vault root");
    let path = scanned
        .files
        .iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))?;
    Some(
        env.org_fs
            .read_to_string(path)
            .await
            .expect("read a vault file the scan just listed"),
    )
}

/// Title comes from cooklang metadata, not the filename, so the assertion
/// below distinguishes "the adapter parsed it" from "something guessed a title
/// off the path".
const PANCAKES_COOK: &str = "\
---
title: Fluffy Pancakes
servings: 4
---
Crack the @eggs{2} into a bowl.

Whisk in the @flour{200%g} and cook for ~{3%minutes}.
";

/// An ordinary org page in the SAME vault: routing must not become
/// cook-shaped, so an org regression shows up as this same test failing.
const NOTES_ORG: &str = "\
* Notes Root
:PROPERTIES:
:ID: notes-root
:END:
** A Note
:PROPERTIES:
:ID: notes-child
:END:
";

/// Red 1 — routing. A `.cook` file in the vault must produce its document and
/// its step blocks in the store, alongside an org file that still ingests.
#[test]
fn a_cook_file_in_the_vault_ingests_beside_org() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_vault_file("Pancakes.cook", PANCAKES_COOK)
            .with_vault_file("Notes.org", NOTES_ORG)
            .build(rt.clone())
            .await
            .expect("a vault holding a `.cook` file must boot");

        // The org leg is unaffected.
        for id in ["notes-root", "notes-child"] {
            assert!(
                env.wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "org block {id} did not sync — widening the format registry broke the org leg"
            );
        }

        // The recipe's document. `CookFormatAdapter::parse` ids the document
        // by its vault-relative path and titles it from cooklang metadata.
        // The recipe's page entity. Titled from the FILENAME, as every page
        // is: the cooklang `title:` metadata reaches the document block the
        // adapter parses, but `sync_document_metadata` is the only seam that
        // could carry it onto the persisted page and the cook adapter does not
        // implement it. Asserted as it behaves, not as it ideally would —
        // carrying the metadata title is recorded as an Inc B follow-up.
        let doc_rows = env
            .test_ctx()
            .query(
                "SELECT id FROM block_raw WHERE content = 'Pancakes'".to_string(),
                QueryLanguage::HolonSql,
                HashMap::new(),
            )
            .await
            .expect("query the recipe document row");
        assert_eq!(
            doc_rows.len(),
            1,
            "no page was minted for Pancakes.cook — nothing routed it into the controller"
        );

        // Its steps. Ids are `<file id>::b::<seq>`, minted by the adapter.
        let step_rows = env
            .test_ctx()
            .query(
                "SELECT id, content FROM block_raw WHERE id LIKE 'block:Pancakes.cook::b::%'"
                    .to_string(),
                QueryLanguage::HolonSql,
                HashMap::new(),
            )
            .await
            .expect("query the recipe step rows");
        assert_eq!(
            step_rows.len(),
            2,
            "expected the recipe's 2 steps in the store, got {}",
            step_rows.len()
        );
        let steps = format!(
            "{:?}",
            step_rows
                .iter()
                .map(|r| r.get("content"))
                .collect::<Vec<_>>()
        );
        // De-sugared cooklang: `@eggs{2}` rendered as "eggs", `~{3%minutes}`
        // as "3 minutes". No org parser produces this from a file with no
        // headlines, so this is the decisive proof that CookFormatAdapter —
        // and not some fallback — parsed the file.
        assert!(
            steps.contains("Crack the eggs into a bowl.")
                && steps.contains("Whisk in the flour and cook for 3 minutes."),
            "step text is not the cook adapter's de-sugared output: {steps:?}"
        );
    });
}

/// Red 2 — the write half. The recipe file is authoritative: a store-side edit
/// of one of its blocks must leave the `.cook` bytes untouched AND must not
/// mint an org-format second home for the same page.
#[test]
fn a_cook_file_is_never_written_back_and_grows_no_second_home() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_vault_file("Pancakes.cook", PANCAKES_COOK)
            .with_vault_file("Notes.org", NOTES_ORG)
            .build(rt.clone())
            .await
            .expect("a vault holding a `.cook` file must boot");

        assert!(
            env.wait_for_block("block:Pancakes.cook::b::0", SYNC_TIMEOUT)
                .await,
            "precondition: the recipe's first step must be in the store"
        );

        // Read through the FileSystem port and address files by NAME: the
        // vault root is an in-memory tree whose paths are canonicalized, so a
        // hand-joined path can miss.
        let before = read_vault_file(&env, "Pancakes.cook")
            .await
            .expect("the recipe file must exist before the edit");

        // Edit a recipe block in the store. In an org document this is exactly
        // what triggers write-back of the owning file.
        env.test_ctx()
            .query(
                "UPDATE block_raw SET content = 'REWRITTEN BY THE STORE' WHERE id = \
                 'block:Pancakes.cook::b::0'"
                    .to_string(),
                QueryLanguage::HolonSql,
                HashMap::new(),
            )
            .await
            .expect("edit a recipe block in the store");

        // Give write-back every chance to happen before asserting it did not.
        env.wait_for_org_files_stable(25, Duration::from_millis(3000))
            .await;

        let after = read_vault_file(&env, "Pancakes.cook")
            .await
            .expect("the recipe file must still exist after the edit");
        assert_eq!(
            before, after,
            "the authoritative `.cook` file was REWRITTEN by write-back — Inc A ships no cooklang \
             renderer, so whatever landed on disk is a reconstruction, not the user's recipe"
        );

        // The second-home hazard: the page-file derivation appends `.org`, so
        // an ungated write-back materializes an org twin beside the recipe
        // instead of refusing.
        assert!(
            read_vault_file(&env, "Pancakes.org").await.is_none(),
            "an org-format SECOND home was minted for a page that already owns Pancakes.cook — \
             the same recipe now has two divergent files"
        );
    });
}

/// A recipe with an ingredient nothing in the pantry covers, so the blocker
/// query has something to name.
const OMELETTE_COOK: &str = "\
---
title: Cheese Omelette
---
Beat the @eggs{3} and fold in the @cheese{50%g}.
";

/// Different BYTES, identical ingredients — the edit that must re-ingest and
/// yet change nothing about the rows.
const PANCAKES_COOK_REWORDED: &str = "\
---
title: Fluffy Pancakes
servings: 4
---
Crack the @eggs{2} into a large mixing bowl.

Whisk in the @flour{200%g} and cook for ~{3%minutes}.
";

/// An ingredient inserted as the FIRST step, pushing every other ingredient
/// down a position.
const PANCAKES_COOK_WITH_BUTTER: &str = "\
---
title: Fluffy Pancakes
servings: 4
---
Melt the @butter{10%g} in the pan.

Crack the @eggs{2} into a bowl.

Whisk in the @flour{200%g} and cook for ~{3%minutes}.
";

/// `Pancakes.cook` after the cook edits it: more flour, and the eggs are gone.
const PANCAKES_COOK_EDITED: &str = "\
---
title: Fluffy Pancakes
servings: 4
---
Whisk in the @flour{250%g} and cook for ~{3%minutes}.
";

fn params(pairs: &[(&str, holon_api::Value)]) -> HashMap<String, holon_api::Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

/// Stock the pantry through the declared type's generic `create` — the same
/// door Inc B's `add` op uses, so this fixture cannot drift from production.
async fn stock(
    env: &holon_integration_tests::TestEnvironment,
    id: &str,
    name: &str,
    quantity: f64,
    unit: Option<&str>,
) {
    env.execute_operation(
        "pantry_item",
        "create",
        params(&[
            ("id", holon_api::Value::String(id.to_string())),
            ("name", holon_api::Value::String(name.to_string())),
            ("quantity", holon_api::Value::Float(quantity)),
            (
                "unit",
                unit.map(|u| holon_api::Value::String(u.to_string()))
                    .unwrap_or(holon_api::Value::Null),
            ),
        ]),
    )
    .await
    .expect("stock the pantry");
}

/// One TEXT column of a result row, or `None` when the column is absent or
/// holds a non-text value.
fn text<'a>(row: &'a holon_api::widget_spec::DataRow, column: &str) -> Option<&'a str> {
    row.get(column).and_then(|v| v.as_string())
}

async fn rows(
    env: &holon_integration_tests::TestEnvironment,
    sql: &str,
) -> Vec<holon_api::widget_spec::DataRow> {
    env.test_ctx()
        .query(sql.to_string(), QueryLanguage::HolonSql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("query {sql:?}: {e:#}"))
}

/// Poll `sql` until it returns `expected` rows, or fail naming what it saw.
/// The ingest write and the matview read are separate transactions, so a bare
/// read races the projection rather than measuring it.
async fn rows_eventually(
    env: &holon_integration_tests::TestEnvironment,
    sql: &str,
    expected: usize,
    what: &str,
) -> Vec<holon_api::widget_spec::DataRow> {
    let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let got = rows(env, sql).await;
        if got.len() == expected {
            return got;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "{what}: expected {expected} rows, got {} — {got:?}",
                got.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Red 3 — the ingest leg. A `.cook` file in the vault must produce `recipe`
/// and `ingredient_use` ROWS, joined by the MINTED `recipe:` id, so the
/// cookable-now query answers over a real vault file rather than only over
/// rows hand-written through the PN surface.
#[test]
fn a_vault_recipe_produces_typed_rows_and_answers_cookable_now() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_vault_file("Pancakes.cook", PANCAKES_COOK)
            .with_vault_file("Omelette.cook", OMELETTE_COOK)
            .with_vault_file("Notes.org", NOTES_ORG)
            .build(rt.clone())
            .await
            .expect("a vault holding `.cook` files must boot");

        assert!(
            env.wait_for_block("block:Pancakes.cook::b::0", SYNC_TIMEOUT)
                .await,
            "precondition: the recipe's first step must be in the store"
        );

        // Both recipes, titled from cooklang metadata rather than the filename.
        let recipes = rows_eventually(
            &env,
            "SELECT id, title, source_path FROM recipe ORDER BY title",
            2,
            "the vault's two `.cook` files must each produce ONE recipe row",
        )
        .await;
        let titles: Vec<Option<&str>> = recipes.iter().map(|r| text(r, "title")).collect();
        assert_eq!(
            titles,
            vec![Some("Cheese Omelette"), Some("Fluffy Pancakes")],
            "recipe titles are not the cooklang metadata titles: {titles:?}"
        );

        // The id rule: the write path prefixes a supplied id with the entity's
        // kebab name, and `ingredient_use.recipe_id` must hold THAT form. A
        // bare id joins to nothing, silently.
        for r in &recipes {
            let id = text(r, "id").expect("recipe row has an id");
            assert!(
                id.starts_with("recipe:"),
                "recipe id {id:?} is not entity-scoped — the join key is the minted form"
            );
        }

        // The join itself, asserted directly: every ingredient use resolves to
        // its recipe. `Pancakes` has eggs + flour, `Omelette` eggs + cheese.
        let joined = rows_eventually(
            &env,
            "SELECT iu.id AS id, r.title AS title, iu.raw_name AS raw_name, iu.quantity AS quantity, \
             iu.unit AS unit, iu.step_index AS step_index \
             FROM ingredient_use iu JOIN recipe r ON r.id = iu.recipe_id \
             ORDER BY r.title, iu.raw_name",
            4,
            "ingredient_use rows must join to their recipe by the minted recipe id",
        )
        .await;
        for r in &joined {
            let id = text(r, "id").expect("ingredient_use row has an id");
            assert!(
                id.starts_with("ingredient-use:"),
                "ingredient_use id {id:?} is not entity-scoped"
            );
        }
        let pairs: Vec<(Option<&str>, Option<&str>)> = joined
            .iter()
            .map(|r| (text(r, "title"), text(r, "raw_name")))
            .collect();
        assert_eq!(
            pairs,
            vec![
                (Some("Cheese Omelette"), Some("cheese")),
                (Some("Cheese Omelette"), Some("eggs")),
                (Some("Fluffy Pancakes"), Some("eggs")),
                (Some("Fluffy Pancakes"), Some("flour")),
            ],
            "the ingredient uses did not land under their own recipes: {pairs:?}"
        );

        // Enough for the pancakes, nothing for the omelette's cheese.
        stock(&env, "p-eggs", "eggs", 12.0, None).await;
        stock(&env, "p-flour", "flour", 500.0, Some("g")).await;

        let cookable = rows_eventually(
            &env,
            &COOKABLE_RECIPES_SQL,
            1,
            "exactly the pancakes must be cookable from the stocked pantry",
        )
        .await;
        assert_eq!(
            text(&cookable[0], "title"),
            Some("Fluffy Pancakes"),
            "the cookable recipe is not the one the pantry covers: {cookable:?}"
        );

        // The disclosure half: the omelette is uncookable BY NAME.
        let blockers = rows_eventually(
            &env,
            &COOK_BLOCKERS_SQL,
            1,
            "the omelette's missing cheese must be named as a blocker",
        )
        .await;
        assert_eq!(text(&blockers[0], "raw_name"), Some("cheese"));
        assert_eq!(
            text(&blockers[0], "reason"),
            Some(CookBlockReason::Missing.as_str()),
            "a pantry holding nothing by that name is a `missing` blocker: {blockers:?}"
        );
    });
}

/// `(id, raw_name)` for every ingredient use, name-ordered — the identity the
/// re-ingest rungs below compare across edits.
async fn use_identities(
    env: &holon_integration_tests::TestEnvironment,
    expected: usize,
    what: &str,
) -> Vec<(String, String)> {
    rows_eventually(
        env,
        "SELECT id, raw_name FROM ingredient_use ORDER BY raw_name",
        expected,
        what,
    )
    .await
    .iter()
    .map(|r| {
        (
            text(r, "id").expect("row has an id").to_string(),
            text(r, "raw_name").expect("row has a raw_name").to_string(),
        )
    })
    .collect()
}

/// Rewrite the recipe on disk and wait for the vault to settle. The content
/// must DIFFER from what is there: an unchanged file is skipped by the
/// controller's content-hash fast path and never re-ingests at all.
async fn re_save(env: &holon_integration_tests::TestEnvironment, content: &str) {
    env.write_org_file("Pancakes.cook", content)
        .await
        .expect("re-save the recipe");
    env.wait_for_org_files_stable(25, Duration::from_millis(3000))
        .await;
}

/// Red 4 — re-ingest is a REPLACEMENT, and ids survive it.
///
/// Three edits, each a different hazard: prose-only (rows must not duplicate),
/// an ingredient inserted ABOVE the others (their ids must not shift onto
/// different ingredients), and an ingredient deleted (no orphan left behind).
#[test]
fn re_ingesting_a_recipe_replaces_its_rows_and_keeps_ingredient_ids() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_vault_file("Pancakes.cook", PANCAKES_COOK)
            .build(rt.clone())
            .await
            .expect("a vault holding a `.cook` file must boot");

        let first = use_identities(&env, 2, "the recipe's two ingredients").await;
        assert_eq!(
            first.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
            vec!["eggs", "flour"]
        );

        // Prose-only edit — the bytes differ so the file genuinely re-ingests,
        // but the ingredients do not. Same rows, same ids, no duplicates.
        re_save(&env, PANCAKES_COOK_REWORDED).await;
        let reworded = use_identities(&env, 2, "a prose-only edit must not duplicate rows").await;
        assert_eq!(
            reworded, first,
            "a prose-only edit re-minted the ingredient rows"
        );
        rows_eventually(
            &env,
            "SELECT id FROM recipe",
            1,
            "nor duplicate the recipe row",
        )
        .await;

        // The reshuffling case: butter is inserted as the FIRST step, so every
        // ingredient below it moves down a position. Positional ids would
        // re-point eggs' id at butter and flour's at eggs — silently, and
        // wrongly, for anything that held one.
        re_save(&env, PANCAKES_COOK_WITH_BUTTER).await;
        let with_butter = use_identities(&env, 3, "the recipe's three ingredients").await;
        assert_eq!(
            with_butter
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["butter", "eggs", "flour"]
        );
        for (id, name) in &first {
            assert!(
                with_butter.contains(&(id.clone(), name.clone())),
                "inserting an ingredient above {name:?} moved its id {id:?} — ids are positional, \
                 so anything holding one now names a different ingredient. After: {with_butter:?}"
            );
        }

        // A deletion: the eggs and the butter are gone, the flour is doubled.
        re_save(&env, PANCAKES_COOK_EDITED).await;
        let after = rows_eventually(
            &env,
            "SELECT id, raw_name, quantity FROM ingredient_use ORDER BY raw_name",
            1,
            "the deleted ingredients left ORPHAN rows — they would block this recipe forever",
        )
        .await;
        assert_eq!(text(&after[0], "raw_name"), Some("flour"));
        assert_eq!(
            after[0].get("quantity").and_then(|v| v.as_f64()),
            Some(250.0),
            "the edited quantity did not reach the row: {after:?}"
        );
        let flour_id = first
            .iter()
            .find(|(_, n)| n == "flour")
            .map(|(id, _)| id.clone())
            .expect("flour was in the first ingest");
        assert_eq!(
            text(&after[0], "id"),
            Some(flour_id.as_str()),
            "the surviving flour row changed id across three edits"
        );
    });
}

/// Red 5 — provenance. Vault ingest DERIVES rows from a file that stays
/// authoritative; it is not a user action. An undo entry for one is worse than
/// useless — the next ingest of the same file writes the row straight back —
/// and a vault of recipes would bury the user's own edits under hundreds of
/// machine entries on every boot.
#[test]
fn vault_ingest_records_no_undo_log_entries() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_vault_file("Pancakes.cook", PANCAKES_COOK)
            .with_vault_file("Omelette.cook", OMELETTE_COOK)
            .build(rt.clone())
            .await
            .expect("a vault holding `.cook` files must boot");

        // Precondition: the rows really were written, so an empty undo log
        // below means "not logged", never "never ran".
        rows_eventually(&env, "SELECT id FROM recipe", 2, "both recipes ingested").await;

        let logged = rows(&env, "SELECT entity_name, op_name FROM operation").await;
        let derived: Vec<String> = logged
            .iter()
            .filter_map(|r| {
                let entity = text(r, "entity_name")?;
                matches!(entity, "recipe" | "ingredient-use" | "ingredient_use")
                    .then(|| format!("{entity}.{}", text(r, "op_name").unwrap_or("?")))
            })
            .collect();
        assert!(
            derived.is_empty(),
            "vault ingest put {} machine-derived write(s) on the undo/redo log: {derived:?}",
            derived.len()
        );
    });
}
