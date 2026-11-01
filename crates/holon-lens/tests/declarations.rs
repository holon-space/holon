//! Declarations the loader refuses, each with a message that names the
//! offending part of the declaration.

use holon_lens::Format;
use holon_lens::FormatError;
use holon_lens::bundled;
use holon_lens::bundled_names;

const TSL: &str = include_str!("../formats/tsl_count.yaml");
const COOK: &str = include_str!("../formats/cooklang_quantity.yaml");

fn refused(yaml: &str) -> String {
    match Format::from_yaml(yaml) {
        Ok(f) => panic!("{f:?} was accepted"),
        Err(e @ FormatError::Declaration { .. }) => e.to_string(),
        Err(e) => panic!("not a declaration error: {e}"),
    }
}

fn changed(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "variant source not found: {from:?}");
    text.replace(from, to)
}

fn assert_has(msg: &str, parts: &[&str]) {
    for p in parts {
        assert!(msg.contains(p), "{p:?} missing in:\n{msg}");
    }
}

#[test]
fn every_bundled_format_compiles() {
    for name in bundled_names() {
        let f = bundled(name).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(f.name(), name);
    }
    let e = bundled("tsl_cuont").unwrap_err().to_string();
    assert_eq!(
        e,
        "unknown format `tsl_cuont` (known: tsl_count, cooklang_quantity)"
    );
}

#[test]
fn overlapping_union_cases() {
    let without_except = changed(
        TSL,
        "    - except:\n        not: \"1\"\n        reason: 'a bare \"1\" means \"no amount\" in That Shopping List'\n        lens:\n          list:",
        "    - list:",
    );
    let e = refused(&without_except);
    assert_has(
        &e,
        &[
            "format `tsl_count`: lens > union: ambiguous — the text \"1\" matches both case 0",
            "cases must not overlap (use `except` or `otherwise`)",
        ],
    );

    let text_as_plain_case = changed(
        COOK,
        "          # like cooklang: anything else is opaque text (\"a pinch\", \"½\", \"1,5\")\n          otherwise:\n            copy:",
        "            - copy:",
    );
    let e = refused(&text_as_plain_case);
    assert_has(
        &e,
        &[
            "lens > union[1] > seq[0] > field `amount` > union: ambiguous",
            "case 3 (lens > union[1] > seq[0] > field `amount` > union[3] > copy)",
        ],
    );
}

#[test]
fn list_items_that_can_be_cut_two_ways() {
    let e = refused(
        "name: x\ntarget: quantity\nlens:\n  list:\n    item: { field: amount, number: {} }\n",
    );
    assert_has(
        &e,
        &[
            "lens > list: ambiguous — the text",
            "can be cut into items two ways",
        ],
    );
}

#[test]
fn sequence_parts_that_can_be_split_two_ways() {
    let e = refused(
        "name: x\ntarget: quantity\nlens:\n  seq:\n    - { field: amount, number: {} }\n    - { field: unit, copy: \"[a-z0-9]+\" }\n",
    );
    assert_has(
        &e,
        &[
            "lens > seq: ambiguous — the text",
            "between part 0 (lens > seq[0] > field `amount`)",
        ],
    );
    let amount_may_end_in_space = changed(
        COOK,
        "            copy: \"[^%\\\\s]([^%]*[^%\\\\s])?\"\n        - optional",
        "            copy: \"[^%\\\\s][^%]*\"\n        - optional",
    );
    let e = refused(&amount_may_end_in_space);
    assert_has(
        &e,
        &["lens > union[1] > seq: ambiguous", "can be split two ways"],
    );
}

#[test]
fn typos_and_bad_regexes() {
    let e =
        refused("name: x\ntarget: quantity\nlens:\n  seq:\n    - { field: amount, numbr: {} }\n");
    assert_has(
        &e,
        &["lens > seq[0] > field `amount`: unknown lens `numbr`"],
    );
    let e = refused("name: x\ntarget: quantity\nlens:\n  copy: \"[a-z\"\n");
    assert_has(
        &e,
        &[
            "lens > copy: bad regex \"[a-z\"",
            "unclosed character class",
        ],
    );
    let e = refused("name: x\ntarget: quantity\nlenz: {}\n");
    assert_has(&e, &["unknown key"]);
    let e = refused("name: x\ntarget: money\nlens: { copy: \".+\" }\n");
    assert_has(&e, &["target", "only `quantity`"]);
}

#[test]
fn a_lens_that_does_not_describe_a_quantity() {
    let e = refused("name: x\ntarget: quantity\nlens: { number: {} }\n");
    assert_has(&e, &["lens does not describe a quantity", "bare number"]);
    let e = refused(
        "name: x\ntarget: quantity\nlens:\n  seq:\n    - { field: amount, number: {} }\n    - { const: \" \" }\n    - { field: unit, number: {} }\n",
    );
    assert_has(&e, &["a unit must be text"]);
    let e = refused(
        "name: x\ntarget: quantity\nlens:\n  seq:\n    - { field: amount, number: {} }\n    - { const: \" \" }\n    - { field: price, copy: \"[a-z]+\" }\n",
    );
    assert_has(&e, &["unknown term field `price`"]);
}

#[test]
fn a_column_type_names_its_format() {
    for name in bundled_names() {
        let format = holon_lens::FieldFormat::named(name).unwrap();
        let declared = format.declared_type();
        assert!(
            declared.contains("TEXT") && !declared.contains("INT"),
            "{declared}"
        );
        assert_eq!(
            holon_lens::FieldFormat::of_declared_type(&declared).unwrap(),
            Some(format.clone())
        );
        assert_eq!(
            holon_lens::FieldFormat::of_declared_type(&declared.to_ascii_uppercase()).unwrap(),
            Some(format)
        );
    }
    for plain in ["TEXT", "INTEGER", "", "TEXT_FMT"] {
        assert_eq!(
            holon_lens::FieldFormat::of_declared_type(plain).unwrap(),
            None
        );
    }
    let unknown = holon_lens::FieldFormat::of_declared_type("TEXT_FMT_nope").unwrap_err();
    assert!(matches!(unknown, FormatError::Unknown { .. }), "{unknown}");
}
