//! Arithmetic on parsed quantities against an independent integer oracle:
//! each amount is drawn as `whole + num/den` and written the way a recipe
//! writes it ("2 1/2", "1/3", "3", "0.25").

use std::cmp::Ordering;
use std::sync::Arc;
use std::sync::LazyLock;

use holon_lens::Format;
use holon_lens::bundled;
use holon_lens::quantity::Amount;
use holon_lens::quantity::Quantity;
use holon_lens::quantity::Rational;
use holon_lens::quantity::Term;
use proptest::prelude::*;

static COOK: LazyLock<Arc<Format>> = LazyLock::new(|| bundled("cooklang_quantity").unwrap());

const UNITS: &[&str] = &["g", "cup", "cups", "fl oz", "Zehen", "EL"];

/// `(text, numerator, denominator)` of a non-negative amount.
fn amount() -> impl Strategy<Value = (String, i128, i128)> {
    prop_oneof![
        (0i128..1000).prop_map(|w| (w.to_string(), w, 1)),
        (1i128..50, 1i128..13).prop_map(|(n, d)| (format!("{n}/{d}"), n, d)),
        (1i128..20, 1i128..13, 1i128..13).prop_map(|(w, n, d)| (
            format!("{w} {n}/{d}"),
            w * d + n,
            d
        )),
        (0i128..1000, 1i128..1000).prop_map(|(w, f)| (format!("{w}.{f:03}"), w * 1000 + f, 1000)),
    ]
}

fn amount_with_unit(unit: &str) -> impl Strategy<Value = (String, i128, i128)> + use<> {
    let unit = unit.to_string();
    (
        amount(),
        prop::sample::select(&["%", " % ", "% "][..]),
        any::<bool>(),
    )
        .prop_map(move |((text, n, d), sep, upper)| {
            let unit = if upper {
                unit.to_uppercase()
            } else {
                unit.clone()
            };
            (format!("{text}{sep}{unit}"), n, d)
        })
}

fn parse(text: &str) -> Quantity {
    COOK.parse(text)
        .unwrap_or_else(|e| panic!("{text:?}: {e}"))
        .unwrap_or_else(|| panic!("{text:?} parsed to no amount"))
}

fn single_exact(q: &Quantity) -> (Rational, Option<String>) {
    match q.terms() {
        [
            Term {
                amount: Amount::Exact(r),
                unit,
            },
        ] => (*r, unit.clone()),
        other => panic!("expected one exact term, got {other:?}"),
    }
}

proptest! {
    #[test]
    fn same_unit_sum_is_the_exact_rational_sum_and_orders_consistently(
        (unit, (a_text, an, ad), (b_text, bn, bd)) in prop::sample::select(UNITS)
            .prop_flat_map(|u| (Just(u), amount_with_unit(u), amount_with_unit(u)))
    ) {
        let a = parse(&a_text);
        let b = parse(&b_text);
        let sum = a.add(&b).unwrap();
        let (total, total_unit) = single_exact(&sum);
        prop_assert_eq!(total, Rational::new(an * bd + bn * ad, ad * bd).unwrap());
        prop_assert_eq!(total_unit.map(|u| u.to_lowercase()), Some(unit.to_lowercase()));

        prop_assert_eq!(a.compare(&b).unwrap(), (an * bd).cmp(&(bn * ad)));
        prop_assert_eq!(b.compare(&a).unwrap(), (bn * ad).cmp(&(an * bd)));
        let expected = if bn == 0 { Ordering::Equal } else { Ordering::Less };
        prop_assert_eq!(a.compare(&sum).unwrap(), expected, "a vs a + b");
    }

    #[test]
    fn different_units_never_merge_or_compare(
        (ua, ub) in (prop::sample::select(UNITS), prop::sample::select(UNITS))
            .prop_filter("different units", |(a, b)| a.to_lowercase() != b.to_lowercase()),
        (a_text, an, ad) in amount(),
        (b_text, bn, bd) in amount(),
    ) {
        let a = parse(&format!("{a_text}%{ua}"));
        let b = parse(&format!("{b_text}%{ub}"));
        let sum = a.add(&b).unwrap();
        let terms: Vec<_> = sum.terms().iter().map(|t| (t.amount.clone(), t.unit.clone())).collect();
        prop_assert_eq!(terms, vec![
            (Amount::Exact(Rational::new(an, ad).unwrap()), Some(ua.to_string())),
            (Amount::Exact(Rational::new(bn, bd).unwrap()), Some(ub.to_string())),
        ]);
        let err = a.compare(&b).unwrap_err().to_string();
        prop_assert!(err.contains(ua) && err.contains(ub), "{}", err);
    }
}
