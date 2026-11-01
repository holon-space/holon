//! Lens laws for the bundled formats.
//! GetPut: print(parse(s), s) == s. PutGet: parse(print(q, s)) == q.
//! CreateGet: parse(print(q)) == q.

use std::sync::Arc;
use std::sync::LazyLock;

use holon_lens::Format;
use holon_lens::bundled;
use holon_lens::quantity::Amount;
use holon_lens::quantity::Quantity;
use holon_lens::quantity::Rational;
use holon_lens::quantity::Term;
use proptest::prelude::*;

static TSL: LazyLock<Arc<Format>> = LazyLock::new(|| bundled("tsl_count").unwrap());
static COOK: LazyLock<Arc<Format>> = LazyLock::new(|| bundled("cooklang_quantity").unwrap());

fn decimal_text(seps: &'static [char]) -> impl Strategy<Value = String> + Clone {
    (
        0u32..3000,
        prop::option::of((prop::sample::select(seps), "[0-9]{1,3}")),
    )
        .prop_map(|(i, f)| match f {
            Some((s, d)) => format!("{i}{s}{d}"),
            None => i.to_string(),
        })
}

const UNITS: &[&str] = &["kg", "Kg", "g", "Zehen", "Stück", "fl oz", "Pck.", "EL"];

fn tsl_terms_text(more: std::ops::Range<usize>) -> impl Strategy<Value = String> {
    let term = (
        decimal_text(&[',', '.']),
        prop::option::of((" {0,2}", prop::sample::select(UNITS))),
    )
        .prop_map(|(n, u)| match u {
            Some((sp, u)) => format!("{n}{sp}{u}"),
            None => n,
        });
    (
        term.clone(),
        prop::collection::vec(
            (prop::sample::select(&[" + ", "+", "  +  ", " +"][..]), term),
            more,
        ),
    )
        .prop_map(|(first, rest)| {
            rest.into_iter()
                .fold(first, |acc, (sep, t)| format!("{acc}{sep}{t}"))
        })
        .prop_filter("bare 1 is the no-amount case", |s| s != "1")
}

fn tsl_text() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => tsl_terms_text(0..4),
        1 => Just("1".to_string()),
        1 => Just(String::new()),
        1 => "[a-zäö][a-zäö ]{0,8}",
    ]
}

fn cook_text() -> impl Strategy<Value = String> {
    let amount = prop_oneof![
        decimal_text(&['.']),
        (1u32..9, 2u32..9).prop_map(|(n, d)| format!("{n}/{d}")),
        (1u32..5, 1u32..9, 2u32..9).prop_map(|(w, n, d)| format!("{w} {n}/{d}")),
        (
            decimal_text(&['.']),
            prop::sample::select(&["-", " - ", "- "][..]),
            decimal_text(&['.'])
        )
            .prop_map(|(a, s, b)| format!("{a}{s}{b}")),
        "[a-z][a-z ]{0,6}[a-z]",
    ];
    let unit = (
        prop::sample::select(&["%", " % ", "% "][..]),
        prop::sample::select(&["g", "cup", "cups", "fl oz", "Zehen"][..]),
    );
    prop_oneof![
        8 => (amount, prop::option::of(unit)).prop_map(|(a, u)| match u {
            Some((s, u)) => format!("{a}{s}{u}"),
            None => a,
        }),
        1 => Just(String::new()),
    ]
}

fn dec(max_decimals: u32) -> impl Strategy<Value = Rational> {
    (0i128..300_000, 0..=max_decimals).prop_map(|(n, k)| Rational::new(n, 10i128.pow(k)).unwrap())
}

fn tsl_quantity() -> impl Strategy<Value = Option<Quantity>> {
    let term = (dec(2), prop::option::of(prop::sample::select(UNITS))).prop_map(|(r, u)| Term {
        amount: Amount::Exact(r),
        unit: u.map(str::to_string),
    });
    let unitless_one = Quantity::one(Amount::Exact(Rational::int(1)), None);
    prop_oneof![
        6 => prop::collection::vec(term, 1..4)
            .prop_map(Quantity::from_terms)
            .prop_filter("a unitless 1 has no TSL form", move |q| q.as_ref() != Some(&unitless_one)),
        1 => Just(None),
        1 => "[a-zäö][a-zäö ]{0,8}".prop_map(|s| Some(Quantity::one(Amount::Text(s), None))),
    ]
}

fn cook_quantity() -> impl Strategy<Value = Option<Quantity>> {
    let amount = prop_oneof![
        dec(3).prop_map(Amount::Exact),
        (1i128..50, 1i128..12).prop_map(|(n, d)| Amount::Exact(Rational::new(n, d).unwrap())),
        (dec(2), dec(2)).prop_map(|(a, b)| Amount::Range(a, b)),
        "[a-z][a-z ]{0,6}[a-z]".prop_map(Amount::Text),
    ];
    let unit = prop::option::of(prop::sample::select(&["g", "cup", "fl oz", "Zehen"][..]));
    prop_oneof![
        8 => (amount, unit).prop_map(|(a, u)| Some(Quantity::one(a, u))),
        1 => Just(None),
    ]
}

fn fail(e: impl std::fmt::Display) -> TestCaseError {
    TestCaseError::fail(e.to_string())
}

fn check_laws(f: &Format, s: &str, q: &Option<Quantity>) -> Result<(), TestCaseError> {
    let parsed = f.parse(s).map_err(fail)?;
    prop_assert_eq!(
        f.print(parsed.as_ref(), Some(s)).map_err(fail)?,
        s,
        "GetPut"
    );
    let put = f.print(q.as_ref(), Some(s)).map_err(fail)?;
    prop_assert_eq!(&f.parse(&put).map_err(fail)?, q, "PutGet via {:?}", put);
    let created = f.print(q.as_ref(), None).map_err(fail)?;
    prop_assert_eq!(
        &f.parse(&created).map_err(fail)?,
        q,
        "CreateGet via {:?}",
        created
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn tsl_laws(s in tsl_text(), q in tsl_quantity()) {
        check_laws(&TSL, &s, &q)?;
    }

    #[test]
    fn cooklang_laws(s in cook_text(), q in cook_quantity()) {
        check_laws(&COOK, &s, &q)?;
    }

    /// Changing one term keeps every other term's bytes, separators included.
    #[test]
    fn tsl_untouched_terms_byte_identical(
        s in tsl_terms_text(1..4),
        i in any::<prop::sample::Index>(),
        r in dec(2),
    ) {
        let q = TSL.parse(&s).unwrap().unwrap();
        let i = i.index(q.terms().len());
        let mut terms = q.terms().to_vec();
        terms[i].amount = Amount::Exact(r);
        let out = TSL.print(Quantity::from_terms(terms).as_ref(), Some(&s)).unwrap();
        let pieces = |t: &str| t.split('+').map(str::to_string).collect::<Vec<_>>();
        let (before, after) = (pieces(&s), pieces(&out));
        prop_assert_eq!(before.len(), after.len());
        for j in (0..before.len()).filter(|&j| j != i) {
            prop_assert_eq!(&before[j], &after[j]);
        }
    }

    /// Rounding is a quotient: PutGet holds up to the declared 2 decimals.
    #[test]
    fn tsl_rounding_put_get(n in 0i128..100_000, d in 1i128..1000) {
        let r = Rational::new(n, d).unwrap();
        let q = Quantity::one(Amount::Exact(r), Some("g"));
        let back = TSL.parse(&TSL.print(Some(&q), None).unwrap()).unwrap();
        prop_assert_eq!(back, Some(Quantity::one(Amount::Exact(r.round(2).unwrap()), Some("g"))));
    }
}
