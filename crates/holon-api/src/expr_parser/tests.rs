use super::*;

#[test]
fn parses_arithmetic_with_precedence() {
    let c = parse("0.001 * (max_position - position)").unwrap();
    // Structural check that `*` binds the parenthesised subtraction.
    match c {
        Computation::Arith {
            op: ArithOp::Mul, ..
        } => {}
        other => panic!("expected top-level Mul, got {other:?}"),
    }
}

#[test]
fn parses_switch_to_case() {
    let c = parse("switch priority { 1.0 => 100.0, 2.0 => 40.0, _ => 1.0 }").unwrap();
    match c {
        Computation::Case {
            scrutinee,
            branches,
            ..
        } => {
            assert_eq!(*scrutinee, Computation::Field("priority".into()));
            assert_eq!(branches.len(), 2);
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn parses_if_else_chain_to_case() {
    let c = parse("if a > b { 1.0 } else if a <= 0.0 { 2.0 } else { 3.0 }").unwrap();
    match c {
        Computation::Case {
            scrutinee,
            branches,
            ..
        } => {
            assert_eq!(*scrutinee, Computation::Lit(Value::Boolean(true)));
            assert_eq!(branches.len(), 2);
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn rejects_duplicate_switch_cases() {
    // Rhai rejects duplicate cases; the subset does too (parse-don't-validate).
    assert!(parse("switch x { 1.0 => 1.0, 1.0 => 2.0, _ => 0.0 }").is_err());
    // But Integer 1 and Float 1.0 are DISTINCT labels (type-strict switch).
    assert!(parse("switch x { 1 => 1.0, 1.0 => 2.0, _ => 0.0 }").is_ok());
}

#[test]
fn a_float_literal_beyond_f64_is_refused_naming_its_text() {
    for (src, text) in [
        ("1e999", "1e999"),
        ("-1.0e400", "1.0e400"),
        ("switch x { 9e999 => 1.0, _ => 0.0 }", "9e999"),
        ("switch x { -9e999 => 1.0, _ => 0.0 }", "9e999"),
    ] {
        match parse(src) {
            Err(e) => assert!(
                e.message.contains(&format!("`{text}`")) && e.message.contains("finite"),
                "{src}: the refusal must name `{text}`, got {e}"
            ),
            Ok(c) => panic!("{src} must be refused, got {c:?}"),
        }
    }
}

#[test]
fn rejects_non_literal_switch_case_label() {
    // Rhai requires constant case labels; `a + b` is not one.
    assert!(parse("switch x { a + b => 1.0, _ => 0.0 }").is_err());
}

#[test]
fn rejects_out_of_subset_constructs() {
    // Calls other than `is_def_var`, statements, bitwise ops -> Err (the
    // caller falls back to Rhai).
    assert!(parse("max(a, b)").is_err());
    assert!(parse("let x = 1").is_err());
    assert!(parse("a & b").is_err());
    assert!(parse("a || b").is_err());
    assert!(parse("is_def_var(role)").is_err());
    assert!(parse("\"unterminated").is_err());
    assert!(parse("\"bad \\q escape\"").is_err());
}

#[test]
fn parses_the_i3_0_shapes() {
    assert_eq!(
        parse("\"hello\"").unwrap(),
        Computation::Lit(Value::String("hello".into()))
    );
    assert_eq!(parse("()").unwrap(), Computation::Lit(Value::Null));
    assert_eq!(
        parse("is_def_var(\"role\")").unwrap(),
        Computation::IsDefined(FieldIdent::parse("role").unwrap())
    );
    assert!(matches!(
        parse("a > 1 && b > 2").unwrap(),
        Computation::And { .. }
    ));
    // Multibyte string content survives the byte-indexed tokenizer.
    assert_eq!(
        parse("\" — \"").unwrap(),
        Computation::Lit(Value::String(" — ".into()))
    );
    // `+` becomes Concat once a string literal appears, and stays Concat
    // through the rest of the left-associated chain.
    assert!(matches!(
        parse("role + \" — \" + email").unwrap(),
        Computation::Concat { .. }
    ));
}
