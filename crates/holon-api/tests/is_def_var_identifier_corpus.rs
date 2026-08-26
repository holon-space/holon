//! I3-0 F2 — `is_def_var`'s argument names a COLUMN, so it must be an
//! identifier and nothing else.
//!
//! `Computation::IsDefined` lowers to `(<name> IS NOT NULL)`, an unquoted
//! identifier position in planted matview DDL. A definedness test is over a
//! column, so quoting the name is not the fix (it would change the shape into a
//! string-literal comparison); constraining the name to the identifier grammar
//! is. The constraint lives in the type — [`FieldIdent`] — so an illegal name
//! cannot be constructed, and the parser is where a source string meets it.
//!
//! The corpus below is the negative pin: each entry reached raw SQL before the
//! `FieldIdent` boundary existed.

use holon_api::computation::Computation;
use holon_api::computation::FieldIdent;
use holon_api::expr_parser;

/// Names that must never reach SQL. The first two are the shapes the verifier
/// observed lowering to injected DDL; the rest close the quote-breakout and
/// comment-terminator families.
const INJECTION_CORPUS: [&str; 6] = [
    r#"is_def_var("x) OR (1=1")"#,
    r#"is_def_var("email' || (SELECT 1) || '")"#,
    r#"is_def_var("x'")"#,
    r#"is_def_var("x --")"#,
    r#"is_def_var("x; DROP TABLE t")"#,
    r#"is_def_var("")"#,
];

#[test]
fn is_def_var_rejects_every_non_identifier_name() {
    for src in INJECTION_CORPUS {
        match expr_parser::parse(src) {
            Err(e) => assert!(
                e.message.contains("identifier"),
                "`{src}` must be rejected AS a non-identifier; got: {}",
                e.message
            ),
            Ok(comp) => {
                let lowered = comp
                    .compile_sql()
                    .map(|f| f.sql)
                    .unwrap_or_else(|e| format!("<did not lower: {e}>"));
                panic!(
                    "`{src}` must be a loud parse error; it parsed and lowered to SQL `{lowered}`"
                );
            }
        }
    }
}

/// The constraint is on the NAME, not on `is_def_var` — ordinary identifiers,
/// including the underscored and digit-bearing ones the tokenizer already
/// accepts for a bare field reference, keep working.
#[test]
fn is_def_var_accepts_ordinary_identifiers() {
    for name in ["role", "_private", "expand_default", "a1", "A_1b"] {
        let comp = expr_parser::parse(&format!(r#"is_def_var("{name}")"#))
            .unwrap_or_else(|e| panic!("`{name}` is a valid identifier: {e}"));
        let ident = FieldIdent::parse(name).expect("valid identifier");
        assert_eq!(comp, Computation::IsDefined(ident));
        assert_eq!(
            comp.compile_sql().expect("lowers").sql,
            format!("({name} IS NOT NULL)")
        );
    }
}

/// The type is the boundary: `FieldIdent` cannot be built from a non-identifier
/// at all, so no construction path — parser, test, or a future caller — can put
/// one into a `Computation`.
#[test]
fn field_ident_is_unconstructible_from_a_non_identifier() {
    for bad in ["x) OR (1=1", "x'", "x --", "", "1st", "a b", "rôle"] {
        assert!(
            FieldIdent::parse(bad).is_err(),
            "`{bad}` must not construct a FieldIdent"
        );
    }
}

/// The sibling surface: `Predicate` also names columns with a raw `String`, and
/// it is FRB-exposed, so a Dart caller reaches `predicate_to_sql` directly. The
/// same constraint is enforced where the predicate becomes SQL text.
#[test]
fn a_predicate_column_that_is_not_an_identifier_does_not_reach_sql() {
    use holon_api::Predicate;
    use holon_api::Value;
    use holon_api::computation::predicate_to_sql;

    let bad = Predicate::Eq {
        field: "x = 1 OR 1".to_string(),
        value: Value::Integer(1),
    };
    assert!(predicate_to_sql(&bad).is_err(), "must not lower");
    // Also through a composite, which recurses.
    assert!(
        predicate_to_sql(&Predicate::Not(Box::new(bad.clone()))).is_err(),
        "Not must propagate the rejection"
    );
    assert!(
        predicate_to_sql(&Predicate::And(vec![
            Predicate::Always,
            Predicate::IsNotNull("ok_col".to_string()),
        ]))
        .is_ok(),
        "ordinary identifiers still lower"
    );
}

/// `Computation::Field` carries a raw `String` and is safe only because the
/// tokenizer restricts identifiers — a constraint a programmatic caller does
/// not inherit. The SQL boundary rejects such a name loudly rather than
/// emitting it into DDL.
#[test]
fn a_field_name_that_is_not_an_identifier_does_not_reach_sql() {
    let comp = Computation::Field("x) OR (1=1".to_string());
    let err = comp
        .compile_sql()
        .expect_err("a non-identifier column name must not lower");
    assert!(
        err.to_string().contains("identifier"),
        "error must name the constraint: {err}"
    );
}
