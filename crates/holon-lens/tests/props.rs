//! Lens laws for the bundled formats.
//! GetPut: print(parse(s), s) == s. PutGet: parse(print(q, s)) == q.
//! CreateGet: parse(print(q)) == q.

use std::sync::Arc;
use std::sync::LazyLock;

use holon_lens::Format;
use holon_lens::FormatError;
use holon_lens::bundled;
use holon_lens::quantity::Amount;
use holon_lens::quantity::Quantity;
use holon_lens::quantity::Rational;
use holon_lens::quantity::Term;
use proptest::prelude::*;

static TSL: LazyLock<Arc<Format>> = LazyLock::new(|| bundled("tsl_count").unwrap());
static COOK: LazyLock<Arc<Format>> = LazyLock::new(|| bundled("cooklang_quantity").unwrap());

/// Digit runs as typed, leading zeros included. `LONG` digits reach past both
/// `i128` (39 digits) and the number language's bound, so texts on each side
/// of it are drawn.
fn digits(max: usize) -> impl Strategy<Value = String> + Clone {
    prop_oneof![
        "[0-9]{1,4}",
        prop::string::string_regex(&format!("[0-9]{{1,{max}}}")).unwrap()
    ]
}

const LONG: usize = 60;

fn decimal_text(seps: &'static [char], max: usize) -> impl Strategy<Value = String> + Clone {
    (
        digits(max),
        prop::option::of((prop::sample::select(seps), digits(max))),
    )
        .prop_map(|(i, f)| match f {
            Some((s, d)) => format!("{i}{s}{d}"),
            None => i.to_string(),
        })
}

const UNITS: &[&str] = &["kg", "Kg", "g", "Zehen", "Stück", "fl oz", "Pck.", "EL"];

fn tsl_terms_text(more: std::ops::Range<usize>, max: usize) -> impl Strategy<Value = String> {
    let term = (
        decimal_text(&[',', '.'], max),
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
        6 => tsl_terms_text(0..4, LONG),
        1 => Just("1".to_string()),
        1 => Just(String::new()),
        1 => "[a-zäö][a-zäö ]{0,8}",
    ]
}

fn cook_text() -> impl Strategy<Value = String> {
    let amount = prop_oneof![
        decimal_text(&['.'], LONG),
        (digits(LONG), " {0,2}/ {0,2}", digits(LONG)).prop_map(|(n, s, d)| format!("{n}{s}{d}")),
        (digits(LONG), " {1,2}", digits(LONG), digits(LONG))
            .prop_map(|(w, s, n, d)| format!("{w}{s}{n}/{d}")),
        (
            decimal_text(&['.'], LONG),
            prop::sample::select(&["-", " - ", "- "][..]),
            decimal_text(&['.'], LONG)
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

/// Below 10^19 the integer part prints inside the number language; a larger
/// value has no text in either format, and put refuses it
/// (put_writes_or_refuses_any_value draws such values).
fn dec(max_decimals: u32) -> impl Strategy<Value = Rational> {
    (0i128..10i128.pow(19), 0..=max_decimals)
        .prop_map(|(n, k)| Rational::new(n, 10i128.pow(k)).unwrap())
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
        // The fraction language's bound: whole part, numerator and denominator below 10^18;
        // put_writes_or_refuses_any_value draws past it.
        (0i128..10i128.pow(18), 1i128..10i128.pow(18))
            .prop_map(|(n, d)| Amount::Exact(Rational::new(n, d).unwrap())),
        (dec(2), dec(2)).prop_map(|(a, b)| Amount::Range(a, b)),
        "[a-z][a-z ]{0,6}[a-z]".prop_map(Amount::Text),
    ];
    let unit = prop::option::of(prop::sample::select(&["g", "cup", "fl oz", "Zehen"][..]));
    prop_oneof![
        8 => (amount, unit).prop_map(|(a, u)| Some(Quantity::one(a, u))),
        1 => Just(None),
    ]
}

/// Values across the whole `i128` range, both signs, with denominators up to
/// 10^18.
fn any_rational() -> impl Strategy<Value = Rational> {
    (any::<bool>(), 1i128..10, 0u32..38, 1i128..10, 0u32..19).prop_map(|(neg, m, e, dm, de)| {
        let n = m * 10i128.pow(e);
        Rational::new(if neg { -n } else { n }, dm * 10i128.pow(de)).unwrap()
    })
}

fn any_quantity(range: bool) -> impl Strategy<Value = Quantity> {
    let exact = any_rational().prop_map(Amount::Exact);
    let amount = if range {
        prop_oneof![
            exact,
            (any_rational(), any_rational()).prop_map(|(a, b)| Amount::Range(a, b))
        ]
        .boxed()
    } else {
        exact.boxed()
    };
    (
        amount,
        prop::option::of(prop::sample::select(&["g", "cup"][..])),
    )
        .prop_map(|(a, u)| Quantity::one(a, u))
}

/// Put writes text the format reads back, or refuses with Unwritable; a value
/// of 10^19 or more has no text in either format.
fn writes_or_refuses(f: &Format, q: &Quantity) -> Result<(), TestCaseError> {
    let big = |r: &Rational| r.num().unsigned_abs() / r.den().unsigned_abs() >= 10u128.pow(19);
    let past = q.terms().iter().any(|t| match &t.amount {
        Amount::Exact(r) => big(r),
        Amount::Range(a, b) => big(a) || big(b),
        Amount::Text(_) => false,
    });
    match f.print(Some(q), None) {
        Ok(s) => {
            prop_assert!(!past, "{q} printed as {s:?} past the bound");
            f.parse(&s).map_err(fail)?;
        }
        Err(FormatError::Unwritable { .. }) => {}
        Err(e) => prop_assert!(false, "{q}: {e}"),
    }
    Ok(())
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
    /// 18 digits: a longer run is outside the number language, so the whole
    /// text reads as opaque text, which has no terms to keep.
    #[test]
    fn tsl_untouched_terms_byte_identical(
        s in tsl_terms_text(1..4, 18),
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

    #[test]
    fn put_writes_or_refuses_any_value(t in any_quantity(false), c in any_quantity(true)) {
        writes_or_refuses(&TSL, &t)?;
        writes_or_refuses(&COOK, &c)?;
    }

    /// Rounding is a quotient: PutGet holds up to the declared 2 decimals.
    #[test]
    fn tsl_rounding_put_get(n in 0i128..10i128.pow(19), d in 1i128..10i128.pow(18)) {
        let r = Rational::new(n, d).unwrap();
        let q = Quantity::one(Amount::Exact(r), Some("g"));
        let back = TSL.parse(&TSL.print(Some(&q), None).unwrap()).unwrap();
        prop_assert_eq!(back, Some(Quantity::one(Amount::Exact(r.round(2).unwrap()), Some("g"))));
    }
}

/// GetPut on texts each format's language accepts: digit runs past `i128`,
/// fraction digits past `10^38`, and a zero denominator.
#[test]
fn every_text_the_format_accepts_reads_back_byte_equal() {
    for (f, s) in [
        (&*TSL, "99999999999999999999999999999999999999999kg"),
        (&*TSL, "1,00000000000000000000000000000000000000001kg"),
        (&*COOK, "1/0%cup"),
        (&*COOK, "99999999999999999999999999999999999999999%g"),
    ] {
        let parsed = f.parse(s).unwrap_or_else(|e| panic!("{s:?}: {e}"));
        assert_eq!(f.print(parsed.as_ref(), Some(s)).unwrap(), s, "GetPut");
    }
}

/// A value of 20 or more integer digits has no text in tsl_count's number
/// language, so put refuses it instead of printing it some other way.
fn tsl_refuses_past_the_bound(n: i128, d: i128) -> Result<(), TestCaseError> {
    let q = Quantity::one(Amount::Exact(Rational::new(n, d).unwrap()), Some("g"));
    let e = TSL.print(Some(&q), None);
    prop_assert!(
        matches!(e, Err(FormatError::Unwritable { .. })),
        "{}/{}: {:?}",
        n,
        d,
        e
    );
    Ok(())
}

proptest! {
    #[test]
    fn put_refuses_a_value_past_the_number_language(
        m in 1i128..10,
        e in 22u32..38,
        d in 1i128..1000,
    ) {
        for (n, d) in [(29890635088163560541127132423768202995, 135), (m * 10i128.pow(e), d)] {
            tsl_refuses_past_the_bound(n, d)?;
        }
    }
}
