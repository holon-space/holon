//! Inc A rung 1 — a `.cook` file ingests into a recipe document + step blocks
//! + typed ingredient uses, through the same `FileFormatAdapter` seam org and
//! the markdown adapters ride.
//!
//! Read-only tier: the write half of the trait must REFUSE, not render wrong
//! bytes (`.cook` files in the vault stay authoritative).

use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_kitchen::CookFormatAdapter;
use holon_kitchen::IngredientUse;
use holon_kitchen::ingredient_uses;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn parse(rel: &str) -> FileFormatParseResult {
    let root = fixtures();
    let path = root.join(rel);
    let content = std::fs::read_to_string(&path).unwrap();
    CookFormatAdapter::new()
        .parse(&path, &content, &EntityUri::no_parent(), &root)
        .unwrap()
}

fn find<'a>(blocks: &'a [Block], needle: &str) -> &'a Block {
    blocks
        .iter()
        .find(|b| b.content.contains(needle))
        .unwrap_or_else(|| panic!("no block containing {needle:?}"))
}

#[test]
fn adapter_claims_the_cook_extension() {
    assert_eq!(CookFormatAdapter::new().extensions(), &["cook"]);
}

#[test]
fn metadata_becomes_the_document_page() {
    let r = parse("pancakes.cook");
    assert!(r.document.is_page());
    assert_eq!(r.document.content, "Buttermilk Pancakes");
    assert_eq!(
        r.document.get_property_str("servings"),
        Some("4".to_string())
    );
    assert_eq!(
        r.document.get_property_str("course"),
        Some("breakfast".to_string())
    );
}

#[test]
fn each_step_becomes_a_block_in_source_order() {
    let r = parse("pancakes.cook");
    let steps: Vec<&Block> = r
        .blocks
        .iter()
        .filter(|b| b.get_property_str("step_number").is_some())
        .collect();
    assert_eq!(steps.len(), 5, "expected 5 steps, got {}", steps.len());
    assert_eq!(
        steps[0].get_property_str("step_number"),
        Some("1".to_string())
    );
    assert!(steps[0].content.starts_with("Whisk the flour"));
    // Every step hangs off the document, not off the previous step.
    for s in &steps {
        assert_eq!(s.parent_id, r.document.id, "step reparented unexpectedly");
    }
}

#[test]
fn step_text_reads_as_prose_with_components_inlined() {
    let r = parse("pancakes.cook");
    let rest = find(&r.blocks, "Rest the batter");
    // Quantities render into the prose; the cooklang sigils never leak through.
    assert!(
        rest.content.contains("15 minutes"),
        "timer lost: {:?}",
        rest.content
    );
    assert!(
        !rest.content.contains('~'),
        "sigil leaked: {:?}",
        rest.content
    );
    let first = find(&r.blocks, "Whisk the");
    assert!(
        !first.content.contains('@'),
        "sigil leaked: {:?}",
        first.content
    );
    assert!(
        !first.content.contains('#'),
        "sigil leaked: {:?}",
        first.content
    );
    assert!(
        first.content.contains("large bowl"),
        "cookware lost: {:?}",
        first.content
    );
}

#[test]
fn ingredients_extract_with_quantity_and_unit() {
    let content = std::fs::read_to_string(fixtures().join("pancakes.cook")).unwrap();
    let uses = ingredient_uses(&content).unwrap();

    let flour = uses.iter().find(|u| u.name == "flour").expect("no flour");
    assert_eq!(flour.quantity, Some(200.0));
    assert_eq!(flour.unit.as_deref(), Some("g"));

    let eggs = uses.iter().find(|u| u.name == "eggs").expect("no eggs");
    assert_eq!(eggs.quantity, Some(2.0));
    assert_eq!(eggs.unit, None, "a bare count must carry no unit");

    // Inc A does NOT bind ingredients to products — that is Inc D. The binding
    // slot exists and is empty, so the unmatched state is visible rather than
    // silently absent.
    assert!(uses.iter().all(|u| u.product_id.is_none()));
}

#[test]
fn an_unquantified_ingredient_keeps_its_name_and_no_quantity() {
    let content = std::fs::read_to_string(fixtures().join("pancakes.cook")).unwrap();
    let uses = ingredient_uses(&content).unwrap();
    let syrup: &IngredientUse = uses
        .iter()
        .find(|u| u.name == "maple syrup")
        .expect("no maple syrup");
    assert_eq!(syrup.quantity, None);
    assert_eq!(syrup.unit, None);
}

#[test]
fn ingredient_uses_carry_the_step_they_appear_in() {
    let content = std::fs::read_to_string(fixtures().join("pancakes.cook")).unwrap();
    let uses = ingredient_uses(&content).unwrap();
    let flour = uses.iter().find(|u| u.name == "flour").unwrap();
    assert_eq!(flour.step_index, 1);
    let butter = uses.iter().find(|u| u.name == "butter").unwrap();
    assert_eq!(butter.step_index, 4);
}

#[test]
fn an_unclosed_quantity_brace_fails_loud_instead_of_losing_the_quantity() {
    // MEASURED (cooklang 0.18.7): the crate accepts `@flour{200%g` and drops
    // the quantity with no error and no warning — `flour` simply arrives
    // amount-less. Silent loss like that would feed Inc D a confidently wrong
    // rollup, so we refuse it at the boundary.
    let err = ingredient_uses("Add the @flour{200%g to the bowl.\n").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("cooklang") && msg.contains("brace"),
        "error should name the format and the cause: {msg}"
    );
}

#[test]
fn a_late_closing_brace_that_swallows_a_component_is_refused() {
    // MEASURED (0.18.7): `@flour{200%g @sugar}` yields ONE ingredient, flour,
    // with unit "g @sugar" — @sugar is gone from the recipe entirely. Braces
    // BALANCE, so a counting guard passes it; only the parsed unit shows it.
    let err = ingredient_uses("Add @flour{200%g @sugar} to the bowl.\n").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("flour") && msg.contains("sigil"),
        "error should name the ingredient and the swallowed component: {msg}"
    );
}

#[test]
fn a_late_closing_brace_that_swallows_prose_is_refused() {
    // MEASURED (0.18.7): unit becomes "g with a". Balanced braces again.
    let err = ingredient_uses("Mix @flour{200%g with a } sign\n").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("flour") && msg.contains("unit"),
        "error should name the ingredient and its bogus unit: {msg}"
    );
}

#[test]
fn a_sigil_swallowed_into_the_value_is_refused() {
    // MEASURED (0.18.7): `@flour{200 @sugar}` puts the sigil in the VALUE —
    // Text("200 @sugar") with NO unit at all — so a unit-only check skips it
    // entirely and @sugar is silently lost.
    let err = ingredient_uses("Add @flour{200 @sugar} to the bowl.\n").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("flour") && msg.contains("sigil"),
        "error should name the ingredient and the swallowed component: {msg}"
    );
}

#[test]
fn a_component_swallowed_by_cookware_is_refused() {
    // MEASURED (0.18.7): `#pot{1 @salt}` gives cookware `pot` the value
    // Text("1 @salt") and the recipe has NO salt ingredient. Cookware braces
    // swallow exactly like ingredient ones.
    //
    // Timers need no companion rung: `~{10 @salt}` is a hard cooklang parse
    // error ("Timer value is text"), so parse_recipe already refuses it.
    let err = ingredient_uses("Use the #pot{1 @salt} now.\n").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("cookware") && msg.contains("pot") && msg.contains("sigil"),
        "error should name the cookware and the swallowed component: {msg}"
    );
}

#[test]
fn a_textual_amount_without_a_sigil_still_parses() {
    // `@salt{a pinch}` is legitimate cooklang: a text amount is fine, only a
    // SIGIL inside one signals a swallow. The value check must not refuse it.
    let uses = ingredient_uses("Season with @salt{a pinch}.\n").unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].name, "salt");
    assert_eq!(uses[0].quantity, None);
}

#[test]
fn cookware_with_a_plain_amount_still_parses() {
    let uses = ingredient_uses("Use #pot{2} and @rice{1%kg}.\n").unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].name, "rice");
}

#[test]
fn a_stray_closing_brace_in_prose_still_parses() {
    // A surplus `}` is ordinary prose. The earlier counting guard REFUSED this
    // legitimate recipe; only an unclosed OPEN brace may be refused.
    let uses = ingredient_uses("Use the #pot{} for @rice{200%g} and note a } sign.\n").unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].unit.as_deref(), Some("g"));
}

#[test]
fn a_two_word_unit_is_accepted() {
    // `fl oz` is a real unit — the word-count bound must not refuse it.
    let uses = ingredient_uses("Add @cream{200%fl oz}.\n").unwrap();
    assert_eq!(uses[0].unit.as_deref(), Some("fl oz"));
}

#[test]
fn a_two_word_swallow_is_a_known_miss() {
    // Stated bound, not an endorsement: the word-count rule catches three words
    // and up, so a two-word swallow still gets through. Pinned so the limit is
    // visible and a future tightening has a place to land.
    let uses = ingredient_uses("Mix @flour{200%g with} more\n").unwrap();
    assert_eq!(uses[0].unit.as_deref(), Some("g with"));
}

#[test]
fn balanced_braces_still_parse() {
    // The brace guard must not refuse legitimate recipes.
    let uses = ingredient_uses("Add the @flour{200%g} to the #bowl{} for ~{2%min}.\n").unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].quantity, Some(200.0));
}

#[test]
fn a_scalar_list_metadata_value_is_kept_not_dropped() {
    // Standard cooklang `tags: [quick, vegan]`. It used to vanish silently.
    let root = fixtures();
    let path = root.join("tagged.cook");
    let content = "---\ntitle: Tagged\ntags: [quick, vegan]\n---\n\nBoil @water{1%l}.\n";
    let r = CookFormatAdapter::new()
        .parse(&path, content, &EntityUri::no_parent(), &root)
        .unwrap();
    assert_eq!(
        r.document.get_property_str("tags"),
        Some("quick, vegan".to_string()),
        "scalar list dropped instead of joined"
    );
}

#[test]
fn an_unrepresentable_metadata_value_is_refused_by_name() {
    let root = fixtures();
    let path = root.join("nested.cook");
    let content = "---\ntitle: Nested\nnutrition:\n  kcal: 200\n---\n\nBoil @water{1%l}.\n";
    let msg = CookFormatAdapter::new()
        .parse(&path, content, &EntityUri::no_parent(), &root)
        .err()
        .expect("a nested metadata value must be refused, not silently skipped")
        .to_string();
    assert!(
        msg.contains("nutrition"),
        "refusal must name the offending key: {msg}"
    );
}

#[test]
fn write_back_is_refused_loudly() {
    let r = parse("pancakes.cook");
    let a = CookFormatAdapter::new();
    let verdict = a.writeback_drops(
        &fixtures().join("pancakes.cook"),
        "",
        "",
        &[],
        &Default::default(),
        &fixtures(),
    );
    assert!(verdict.is_err(), "read-only adapter must refuse write-back");
    let _ = r;
}
