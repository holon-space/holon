//! The engine value type `quantity`: one or more terms, each an exact amount
//! with an optional unit, such as `0,5 kg + 2 Zehen`.
//!
//! "No amount" is not a `Quantity`; it is the absent value
//! (`Option<Quantity>::None`). Two operations treat it differently, by design
//! of D62.a and of SQL `SUM`: [`Quantity::plus`] (missing in, missing out) and
//! [`Quantity::sum`] (missing values are skipped).

use std::cmp::Ordering;
use std::fmt;

/// An exact rational number in lowest terms with a positive denominator.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i128,
    den: i128,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuantityError {
    #[error("{num}/0 is not a number")]
    ZeroDenominator { num: i128 },
    #[error("{0} overflows the exact number range")]
    Overflow(String),
    #[error("{text:?} is not a decimal number with separators {separators:?}")]
    NotDecimal { text: String, separators: String },
    #[error("cannot subtract {0}: only exact amounts subtract")]
    NotSubtractable(String),
    #[error("unknown unit {0:?}: conversion knows mg, g, kg, ml, cl, dl, l")]
    UnknownUnit(String),
    #[error("{amount} has no unit, so it cannot be converted to {to}")]
    NoUnit { amount: String, to: String },
    #[error("{left} and {right} do not compare: only one exact amount of the same unit compares")]
    Incomparable { left: String, right: String },
    #[error("{from} ({from_dimension}) does not convert to {to} ({to_dimension})")]
    Incompatible {
        from: String,
        from_dimension: &'static str,
        to: String,
        to_dimension: &'static str,
    },
}

type Result<T> = std::result::Result<T, QuantityError>;

fn gcd(a: i128, b: i128) -> i128 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

impl Rational {
    pub fn new(num: i128, den: i128) -> Result<Rational> {
        if den == 0 {
            return Err(QuantityError::ZeroDenominator { num });
        }
        let g = gcd(num, den).max(1);
        let sign = if den < 0 { -1 } else { 1 };
        Ok(Rational {
            num: sign * (num / g),
            den: sign * (den / g),
        })
    }

    pub fn int(n: i128) -> Rational {
        Rational { num: n, den: 1 }
    }

    pub fn num(self) -> i128 {
        self.num
    }

    pub fn den(self) -> i128 {
        self.den
    }

    pub fn checked_add(self, o: Rational) -> Result<Rational> {
        let overflow = || QuantityError::Overflow(format!("{self} + {o}"));
        let n = self
            .num
            .checked_mul(o.den)
            .and_then(|a| o.num.checked_mul(self.den).and_then(|b| a.checked_add(b)))
            .ok_or_else(overflow)?;
        Rational::new(n, self.den.checked_mul(o.den).ok_or_else(overflow)?)
    }

    pub fn checked_mul(self, o: Rational) -> Result<Rational> {
        let overflow = || QuantityError::Overflow(format!("{self} × {o}"));
        Rational::new(
            self.num.checked_mul(o.num).ok_or_else(overflow)?,
            self.den.checked_mul(o.den).ok_or_else(overflow)?,
        )
    }

    pub fn compare(self, o: Rational) -> Result<Ordering> {
        let overflow = || QuantityError::Overflow(format!("comparing {self} with {o}"));
        let a = self.num.checked_mul(o.den).ok_or_else(overflow)?;
        let b = o.num.checked_mul(self.den).ok_or_else(overflow)?;
        Ok(a.cmp(&b))
    }

    pub fn is_positive(self) -> bool {
        self.num > 0
    }

    pub fn is_integer(self) -> bool {
        self.den == 1
    }

    /// Digits with at most one decimal separator, which is any char of
    /// `separators`.
    pub fn parse_decimal(text: &str, separators: &str) -> Result<Rational> {
        let not_decimal = || QuantityError::NotDecimal {
            text: text.to_string(),
            separators: separators.to_string(),
        };
        let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let (int, frac) = match text.find(|c| separators.contains(c)) {
            Some(i) => {
                let sep_len = text[i..].chars().next().expect("found at i").len_utf8();
                let frac = &text[i + sep_len..];
                if !all_digits(frac) {
                    return Err(not_decimal());
                }
                (&text[..i], frac)
            }
            None => (text, ""),
        };
        if !all_digits(int) {
            return Err(not_decimal());
        }
        let n: i128 = format!("{int}{frac}").parse().map_err(|_| not_decimal())?;
        let den = 10i128
            .checked_pow(frac.len() as u32)
            .ok_or_else(|| QuantityError::Overflow(format!("{text:?} has too many decimals")))?;
        Rational::new(n, den)
    }

    /// The value as decimal digits without separator plus the number of
    /// fraction digits, when it has an exact decimal form with at most `max`
    /// fraction digits.
    pub fn to_decimal(self, max: u32) -> Option<(String, u32)> {
        (0..=max).find_map(|k| {
            let scaled = self.num.checked_mul(10i128.checked_pow(k)?)?;
            (scaled % self.den == 0).then(|| ((scaled / self.den).to_string(), k))
        })
    }

    /// Rounds half away from zero to `k` fraction digits.
    pub fn round(self, k: u32) -> Result<Rational> {
        let overflow = || QuantityError::Overflow(format!("rounding {self} to {k} decimals"));
        let p = 10i128.checked_pow(k).ok_or_else(overflow)?;
        let twice = self
            .num
            .checked_mul(p)
            .and_then(|x| x.checked_mul(2))
            .ok_or_else(overflow)?
            / self.den;
        Rational::new((twice + twice.signum()) / 2, p)
    }
}

impl std::ops::Neg for Rational {
    type Output = Rational;

    fn neg(self) -> Rational {
        Rational {
            num: -self.num,
            den: self.den,
        }
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl fmt::Debug for Rational {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self}")
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Amount {
    Exact(Rational),
    Range(Rational, Rational),
    /// Opaque text such as "a pinch": never added, never scaled.
    Text(String),
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Amount::Exact(r) => write!(f, "{r}"),
            Amount::Range(a, b) => write!(f, "{a}..{b}"),
            Amount::Text(t) => write!(f, "{t:?}"),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Term {
    pub amount: Amount,
    /// As written. Compared case-insensitively when terms are added.
    pub unit: Option<String>,
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.unit {
            Some(u) => write!(f, "{} {u}", self.amount),
            None => write!(f, "{}", self.amount),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Quantity {
    terms: Vec<Term>,
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let terms: Vec<String> = self.terms.iter().map(Term::to_string).collect();
        write!(f, "{}", terms.join(" + "))
    }
}

fn same_unit(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => false,
    }
}

/// (dimension, factor to the dimension's base unit).
fn unit_dimension(unit: &str) -> Result<(&'static str, Rational)> {
    let (dimension, num, den) = match unit.to_lowercase().as_str() {
        "mg" => ("mass", 1, 1000),
        "g" => ("mass", 1, 1),
        "kg" => ("mass", 1000, 1),
        "ml" => ("volume", 1, 1000),
        "cl" => ("volume", 1, 100),
        "dl" => ("volume", 1, 10),
        "l" => ("volume", 1, 1),
        _ => return Err(QuantityError::UnknownUnit(unit.to_string())),
    };
    Ok((dimension, Rational::new(num, den)?))
}

impl Quantity {
    /// `None` for no terms: an empty term list is the absent value.
    pub fn from_terms(terms: Vec<Term>) -> Option<Quantity> {
        (!terms.is_empty()).then_some(Quantity { terms })
    }

    pub fn one(amount: Amount, unit: Option<&str>) -> Quantity {
        Quantity {
            terms: vec![Term {
                amount,
                unit: unit.map(str::to_string),
            }],
        }
    }

    pub fn terms(&self) -> &[Term] {
        &self.terms
    }

    /// Sums terms of the same unit (`Kg` = `kg`). Other units and text terms
    /// stay side by side; convertible units merge only through
    /// [`Self::convert`].
    pub fn add(&self, other: &Quantity) -> Result<Quantity> {
        let mut out = self.clone();
        for t in &other.terms {
            let slot = out.terms.iter_mut().find(|s| {
                same_unit(&s.unit, &t.unit)
                    && !matches!(s.amount, Amount::Text(_))
                    && !matches!(t.amount, Amount::Text(_))
            });
            match slot {
                Some(s) => s.amount = add_amount(&s.amount, &t.amount)?,
                None => out.terms.push(t.clone()),
            }
        }
        Ok(out)
    }

    /// Orders two single exact amounts of the same unit (`Kg` = `kg`).
    /// Refuses any other pair, so different units never compare by number.
    pub fn compare(&self, other: &Quantity) -> Result<Ordering> {
        if let ([a], [b]) = (&self.terms[..], &other.terms[..])
            && same_unit(&a.unit, &b.unit)
            && let (Amount::Exact(x), Amount::Exact(y)) = (&a.amount, &b.amount)
        {
            return x.compare(*y);
        }
        Err(QuantityError::Incomparable {
            left: self.to_string(),
            right: other.to_string(),
        })
    }

    /// `a + b` in an expression: missing in, missing out (D62.a).
    pub fn plus(a: Option<&Quantity>, b: Option<&Quantity>) -> Result<Option<Quantity>> {
        match (a, b) {
            (Some(a), Some(b)) => a.add(b).map(Some),
            _ => Ok(None),
        }
    }

    /// Aggregation over contributions, like SQL `SUM`: missing values are
    /// skipped, and only an all-missing input sums to missing.
    pub fn sum<'a>(
        items: impl IntoIterator<Item = Option<&'a Quantity>>,
    ) -> Result<Option<Quantity>> {
        items
            .into_iter()
            .flatten()
            .try_fold(None, |acc: Option<Quantity>, q| match acc {
                None => Ok(Some(q.clone())),
                Some(acc) => acc.add(q).map(Some),
            })
    }

    pub fn sub(&self, other: &Quantity) -> Result<Quantity> {
        let terms = other
            .terms
            .iter()
            .map(|t| match &t.amount {
                Amount::Exact(r) => Ok(Term {
                    amount: Amount::Exact(-*r),
                    unit: t.unit.clone(),
                }),
                _ => Err(QuantityError::NotSubtractable(t.to_string())),
            })
            .collect::<Result<_>>()?;
        self.add(&Quantity { terms })
    }

    /// Multiplies every exact amount and range; text terms stay as they are.
    pub fn scale(&self, factor: Rational) -> Result<Quantity> {
        let terms = self
            .terms
            .iter()
            .map(|t| {
                let amount = match &t.amount {
                    Amount::Exact(r) => Amount::Exact(r.checked_mul(factor)?),
                    Amount::Range(a, b) => {
                        Amount::Range(a.checked_mul(factor)?, b.checked_mul(factor)?)
                    }
                    Amount::Text(s) => Amount::Text(s.clone()),
                };
                Ok(Term {
                    amount,
                    unit: t.unit.clone(),
                })
            })
            .collect::<Result<_>>()?;
        Ok(Quantity { terms })
    }

    /// Drops every term whose amount is zero or below; `None` when none is
    /// left.
    pub fn drop_non_positive(&self) -> Option<Quantity> {
        let keep = |a: &Amount| match a {
            Amount::Exact(r) => r.is_positive(),
            Amount::Range(_, hi) => hi.is_positive(),
            Amount::Text(_) => true,
        };
        Quantity::from_terms(
            self.terms
                .iter()
                .filter(|t| keep(&t.amount))
                .cloned()
                .collect(),
        )
    }

    /// Every term converted to `to` and summed. Refuses a term with no unit,
    /// an unknown unit, or another dimension.
    pub fn convert(&self, to: &str) -> Result<Quantity> {
        let (to_dimension, to_factor) = unit_dimension(to)?;
        let mut sum: Option<Quantity> = None;
        for t in &self.terms {
            let from = t.unit.as_deref().ok_or_else(|| QuantityError::NoUnit {
                amount: t.amount.to_string(),
                to: to.to_string(),
            })?;
            let (from_dimension, from_factor) = unit_dimension(from)?;
            if from_dimension != to_dimension {
                return Err(QuantityError::Incompatible {
                    from: from.to_string(),
                    from_dimension,
                    to: to.to_string(),
                    to_dimension,
                });
            }
            let k = from_factor.checked_mul(Rational::new(to_factor.den(), to_factor.num())?)?;
            let mut converted = Quantity {
                terms: vec![t.clone()],
            }
            .scale(k)?;
            converted.terms[0].unit = Some(to.to_string());
            sum = Some(match sum {
                None => converted,
                Some(s) => s.add(&converted)?,
            });
        }
        Ok(sum.expect("a quantity has at least one term"))
    }
}

/// Never called with a text amount: [`Quantity::add`] keeps those apart.
fn add_amount(a: &Amount, b: &Amount) -> Result<Amount> {
    Ok(match (a, b) {
        (Amount::Exact(x), Amount::Exact(y)) => Amount::Exact(x.checked_add(*y)?),
        (Amount::Exact(x), Amount::Range(lo, hi)) | (Amount::Range(lo, hi), Amount::Exact(x)) => {
            Amount::Range(lo.checked_add(*x)?, hi.checked_add(*x)?)
        }
        (Amount::Range(a, b), Amount::Range(c, d)) => {
            Amount::Range(a.checked_add(*c)?, b.checked_add(*d)?)
        }
        (Amount::Text(_), _) | (_, Amount::Text(_)) => {
            unreachable!("text amounts are kept apart by Quantity::add")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: i128, d: i128) -> Rational {
        Rational::new(n, d).unwrap()
    }

    fn ex(n: i128, d: i128, unit: Option<&str>) -> Quantity {
        Quantity::one(Amount::Exact(r(n, d)), unit)
    }

    #[test]
    fn add_merges_equal_units_case_insensitively_and_keeps_others_apart() {
        let total = ex(1, 2, Some("Kg")).add(&ex(1, 4, Some("kg"))).unwrap();
        assert_eq!(total, ex(3, 4, Some("Kg")));
        let mixed = ex(300, 1, Some("g")).add(&ex(1, 2, Some("kg"))).unwrap();
        assert_eq!(mixed.terms().len(), 2);
        assert_eq!(mixed.convert("kg").unwrap(), ex(4, 5, Some("kg")));
        let pinch = Quantity::one(Amount::Text("a pinch".into()), None);
        assert_eq!(pinch.add(&pinch).unwrap().terms().len(), 2);
    }

    #[test]
    fn compare_refuses_other_units_naming_both() {
        assert_eq!(
            ex(1, 2, Some("Kg")).compare(&ex(3, 4, Some("kg"))).unwrap(),
            Ordering::Less
        );
        let err = ex(2, 1, Some("kg"))
            .compare(&ex(4, 1, Some("Zehen")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("kg") && err.contains("Zehen"), "{err}");
        assert!(ex(1, 1, None).compare(&ex(1, 1, Some("g"))).is_err());
    }

    #[test]
    fn plus_propagates_missing_and_sum_skips_it() {
        let a = ex(1, 1, Some("g"));
        assert_eq!(Quantity::plus(Some(&a), None).unwrap(), None);
        assert_eq!(Quantity::plus(None, Some(&a)).unwrap(), None);
        assert_eq!(
            Quantity::sum([Some(&a), None, Some(&a)]).unwrap(),
            Some(ex(2, 1, Some("g")))
        );
        assert_eq!(Quantity::sum([None, None]).unwrap(), None);
    }

    #[test]
    fn exact_arithmetic_and_refusals() {
        assert_eq!(r(1, 10).checked_add(r(2, 10)).unwrap(), r(3, 10));
        assert_eq!(
            ex(500, 1, Some("g")).scale(r(2, 3)).unwrap(),
            ex(1000, 3, Some("g"))
        );
        assert_eq!(
            ex(200, 1, Some("g"))
                .sub(&ex(300, 1, Some("g")))
                .unwrap()
                .drop_non_positive(),
            None
        );
        assert!(matches!(
            ex(1, 1, Some("g")).convert("l"),
            Err(QuantityError::Incompatible { .. })
        ));
        assert!(matches!(
            Rational::int(i128::MAX).checked_add(Rational::int(1)),
            Err(QuantityError::Overflow(_))
        ));
        assert_eq!(r(1000, 3).round(2).unwrap(), r(33333, 100));
        assert_eq!(r(-5, 1000).round(2).unwrap(), r(-1, 100));
    }

    #[test]
    fn parse_decimal_accepts_one_separator_only() {
        assert_eq!(Rational::parse_decimal("1,5", ",.").unwrap(), r(3, 2));
        assert_eq!(Rational::parse_decimal("0.50", ".").unwrap(), r(1, 2));
        for bad in ["1,5,0", "", ",5", "1,", "a"] {
            assert!(Rational::parse_decimal(bad, ",.").is_err(), "{bad:?}");
        }
    }
}
