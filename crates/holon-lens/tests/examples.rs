use std::sync::Arc;

use holon_lens::Format;
use holon_lens::FormatError;
use holon_lens::bundled;
use holon_lens::quantity::Amount;
use holon_lens::quantity::Quantity;
use holon_lens::quantity::Rational;
use holon_lens::quantity::Term;

fn tsl() -> Arc<Format> {
    bundled("tsl_count").unwrap()
}

fn cook() -> Arc<Format> {
    bundled("cooklang_quantity").unwrap()
}

fn r(n: i128, d: i128) -> Rational {
    Rational::new(n, d).unwrap()
}

fn ex(n: i128, d: i128, u: Option<&str>) -> Quantity {
    Quantity::one(Amount::Exact(r(n, d)), u)
}

fn some(q: Quantity) -> Option<Quantity> {
    Some(q)
}

fn with_terms(q: &Quantity, edit: impl FnOnce(&mut Vec<Term>)) -> Quantity {
    let mut terms = q.terms().to_vec();
    edit(&mut terms);
    Quantity::from_terms(terms).unwrap()
}

#[test]
fn tsl_captured_values_parse() {
    let f = tsl();
    assert_eq!(f.parse("2kg").unwrap(), some(ex(2, 1, Some("kg"))));
    assert_eq!(f.parse("500g").unwrap(), some(ex(500, 1, Some("g"))));
    assert_eq!(f.parse("4 Zehen").unwrap(), some(ex(4, 1, Some("Zehen"))));
    assert_eq!(f.parse("1,5Kg").unwrap(), some(ex(3, 2, Some("Kg"))));
    assert_eq!(f.parse("1").unwrap(), None);
    assert_eq!(f.parse("4").unwrap(), some(ex(4, 1, None)));
    assert_eq!(
        f.parse("viel").unwrap(),
        some(Quantity::one(Amount::Text("viel".into()), None))
    );
    assert_eq!(
        f.parse("0,5Kg + 2 Zehen").unwrap().unwrap().terms().len(),
        2
    );
}

#[test]
fn byte_for_byte_unchanged_term() {
    let f = tsl();
    let orig = "0,5Kg + 2 Zehen";
    let q = f.parse(orig).unwrap().unwrap();
    let q3 = with_terms(&q, |t| t[1].amount = Amount::Exact(Rational::int(3)));
    assert_eq!(f.print(Some(&q3), Some(orig)).unwrap(), "0,5Kg + 3 Zehen");

    let odd = "0,5Kg  +  2 Zehen";
    let q = f.parse(odd).unwrap().unwrap();
    let q = with_terms(&q, |t| t[0].amount = Amount::Exact(r(3, 4)));
    assert_eq!(f.print(Some(&q), Some(odd)).unwrap(), "0,75Kg  +  2 Zehen");

    let q = f.parse(orig).unwrap().unwrap();
    let only_cloves = with_terms(&q, |t| {
        t.remove(0);
    });
    assert_eq!(f.print(Some(&only_cloves), Some(orig)).unwrap(), "2 Zehen");

    assert_eq!(
        f.print(Some(&ex(3, 4, Some("kg"))), Some("0.5kg")).unwrap(),
        "0.75kg"
    );
}

#[test]
fn arithmetic_through_the_format() {
    let f = tsl();
    let orig = "0,5Kg + 2 Zehen";
    let total = f.parse(orig).unwrap().unwrap();
    let sum = total
        .add(&ex(250, 1, Some("g")))
        .unwrap()
        .add(&ex(1, 4, Some("kg")))
        .unwrap();
    assert_eq!(
        f.print(Some(&sum), Some(orig)).unwrap(),
        "0,75Kg + 2 Zehen + 250g"
    );

    let g500 = f.parse("500g").unwrap().unwrap();
    assert_eq!(
        f.print(Some(&g500.scale(r(6, 4)).unwrap()), Some("500g"))
            .unwrap(),
        "750g"
    );
    assert_eq!(
        f.print(Some(&g500.scale(r(2, 3)).unwrap()), Some("500g"))
            .unwrap(),
        "333,33g"
    );

    assert_eq!(Quantity::plus(Some(&total), None).unwrap(), None);
    assert_eq!(
        Quantity::sum([Some(&total), None]).unwrap(),
        Some(total.clone())
    );

    let below_zero = f
        .parse("200g")
        .unwrap()
        .unwrap()
        .sub(&ex(300, 1, Some("g")))
        .unwrap()
        .drop_non_positive();
    assert_eq!(f.print(below_zero.as_ref(), None).unwrap(), "1");

    let mixed = ex(300, 1, Some("g")).add(&ex(1, 2, Some("kg"))).unwrap();
    assert_eq!(f.print(Some(&mixed), None).unwrap(), "300g + 0,5kg");
    assert_eq!(
        f.print(Some(&mixed.convert("kg").unwrap()), None).unwrap(),
        "0,8kg"
    );
}

#[test]
fn tsl_refusals_name_the_declared_reason_only() {
    let f = tsl();
    let e = f
        .print(Some(&ex(1, 1, None)), None)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains(r#"a bare "1" means "no amount" in That Shopping List"#),
        "{e}"
    );
    assert_eq!(
        e.lines().count(),
        2,
        "only the declared refusal is listed:\n{e}"
    );

    let range = Quantity::one(Amount::Range(r(2, 1), r(3, 1)), Some("kg"));
    let e = f.print(Some(&range), None).unwrap_err();
    assert!(matches!(e, FormatError::Unwritable { .. }), "{e}");
}

#[test]
fn a_stored_text_outside_the_format_is_refused_by_name() {
    let e = cook().parse("200%").unwrap_err().to_string();
    assert!(e.contains(r#"it ends too early after "200%""#), "{e}");
    let e = cook().parse("2%g%").unwrap_err().to_string();
    assert!(e.contains(r#"it goes wrong at "%" (after "2%g")"#), "{e}");
    let e = cook()
        .print(Some(&ex(2, 1, Some("g"))), Some("200%"))
        .unwrap_err();
    assert!(matches!(e, FormatError::NoMatch { .. }), "{e}");
}

#[test]
fn a_cleared_tsl_amount_round_trips_byte_equal() {
    let empty = tsl().parse("").unwrap();
    assert_eq!(
        empty,
        some(Quantity::one(Amount::Text(String::new()), None))
    );
    assert_eq!(tsl().print(empty.as_ref(), None).unwrap(), "");
    assert_eq!(tsl().print(empty.as_ref(), Some("")).unwrap(), "");
    assert_eq!(tsl().print(empty.as_ref(), Some("2kg")).unwrap(), "");
}

#[test]
fn cooklang_dialect() {
    let f = cook();
    assert_eq!(f.parse("200%g").unwrap(), some(ex(200, 1, Some("g"))));
    assert_eq!(f.parse("1/2%cup").unwrap(), some(ex(1, 2, Some("cup"))));
    assert_eq!(
        f.parse("1 1/2 % cups").unwrap(),
        some(ex(3, 2, Some("cups")))
    );
    assert_eq!(
        f.parse("2-3%cloves").unwrap(),
        some(Quantity::one(
            Amount::Range(r(2, 1), r(3, 1)),
            Some("cloves")
        ))
    );
    assert_eq!(
        f.parse("a pinch").unwrap(),
        some(Quantity::one(Amount::Text("a pinch".into()), None))
    );
    assert_eq!(f.parse("").unwrap(), None);

    let scaled = |s: &str, k: Rational| {
        let q = f.parse(s).unwrap().unwrap().scale(k).unwrap();
        f.print(Some(&q), Some(s)).unwrap()
    };
    assert_eq!(scaled("1/2%cup", r(3, 1)), "1 1/2%cup");
    assert_eq!(scaled("0.5%cup", r(3, 1)), "1.5%cup");
    assert_eq!(scaled("1%cup", r(1, 3)), "1/3%cup");
    assert_eq!(scaled("2 - 3 % cloves", r(2, 1)), "4 - 6 % cloves");

    let two_terms = ex(1, 1, Some("g")).add(&ex(1, 1, Some("kg"))).unwrap();
    assert!(f.print(Some(&two_terms), None).is_err());
}
