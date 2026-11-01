//! A declared text format bound to the `quantity` value type.
//!
//! Tree convention, checked when the format loads: `null` = no amount; a
//! record `{amount, unit?}` = one term; a list of such records = terms; a bare
//! string = one text term. `amount` is a number, a `{from, to}` range, or a
//! string.

use std::collections::BTreeMap;

use crate::lens;
use crate::lens::Fail;
use crate::lens::Node;
use crate::lens::Tree;
use crate::lens::TreeType;
use crate::quantity::Amount;
use crate::quantity::Quantity;
use crate::quantity::Term;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("format `{name}`: {detail}")]
    Declaration { name: String, detail: String },
    #[error("unknown format `{name}` (known: {known})")]
    Unknown { name: String, known: String },
    #[error("{format}: {detail}")]
    NoMatch { format: String, detail: String },
    #[error("{format}: cannot write {value}:\n{detail}")]
    Unwritable {
        format: String,
        value: String,
        detail: String,
    },
}

pub struct Format {
    name: String,
    node: Node,
}

impl std::fmt::Debug for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Format({})", self.name)
    }
}

impl Format {
    pub fn from_yaml(text: &str) -> Result<Format, FormatError> {
        let declaration = |name: &str, detail: String| FormatError::Declaration {
            name: name.to_string(),
            detail,
        };
        let y: serde_yaml::Value =
            serde_yaml::from_str(text).map_err(|e| declaration("?", e.to_string()))?;
        let name = y["name"]
            .as_str()
            .ok_or_else(|| declaration("?", "the declaration has no `name`".into()))?
            .to_string();
        let keys = y.as_mapping().expect("indexing by `name` succeeded").keys();
        for k in keys {
            if !matches!(k.as_str(), Some("name" | "target" | "lens")) {
                return Err(declaration(
                    &name,
                    format!("unknown key {k:?} (allowed: name, target, lens)"),
                ));
            }
        }
        if y["target"].as_str() != Some("quantity") {
            return Err(declaration(
                &name,
                format!(
                    "target {:?} is not supported (only `quantity`)",
                    y["target"]
                ),
            ));
        }
        let node = lens::compile(&y["lens"]).map_err(|e| declaration(&name, e))?;
        fits_quantity(&node.tree_type())
            .map_err(|e| declaration(&name, format!("lens does not describe a quantity: {e}")))?;
        Ok(Format { name, node })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// `None` is the format's "no amount" text.
    pub fn parse(&self, s: &str) -> Result<Option<Quantity>, FormatError> {
        let t = lens::get(&self.node, s).map_err(|detail| FormatError::NoMatch {
            format: self.name.clone(),
            detail,
        })?;
        Ok(quantity_of(&t))
    }

    /// Prints `q`; with `original`, every chunk whose value did not change
    /// keeps its bytes.
    pub fn print(
        &self,
        q: Option<&Quantity>,
        original: Option<&str>,
    ) -> Result<String, FormatError> {
        if let Some(o) = original {
            self.parse(o)?;
        }
        let mut fails: Vec<Fail> = vec![];
        for t in trees_of(q) {
            match lens::put(&self.node, &t, original) {
                Ok(s) => return Ok(s),
                Err(f) => fails.extend(f),
            }
        }
        if fails.iter().any(|f| f.refused) {
            fails.retain(|f| f.refused);
        }
        let mut lines: Vec<String> = fails
            .iter()
            .map(|f| format!("  - {}: {}", f.path, f.msg))
            .collect();
        lines.dedup();
        Err(FormatError::Unwritable {
            format: self.name.clone(),
            value: q.map_or_else(|| "no amount".to_string(), Quantity::to_string),
            detail: lines.join("\n"),
        })
    }

    pub fn dfa_states(&self) -> usize {
        self.node.lang.states()
    }
}

fn fits_quantity(t: &TreeType) -> Result<(), String> {
    match t {
        TreeType::Null | TreeType::Str => Ok(()),
        TreeType::Rec(_) => fits_term(t),
        TreeType::List(item) => fits_term(item),
        TreeType::Union(cases) => cases.iter().try_for_each(fits_quantity),
        TreeType::Num => Err("a bare number at the top; name it with `field: amount`".into()),
    }
}

fn fits_term(t: &TreeType) -> Result<(), String> {
    let TreeType::Rec(fields) = t else {
        return Err(format!(
            "a term must be a record {{amount, unit}}, got {t:?}"
        ));
    };
    if let Some(k) = fields.keys().find(|k| *k != "amount" && *k != "unit") {
        return Err(format!(
            "unknown term field `{k}` (a term has `amount` and `unit`)"
        ));
    }
    match fields.get("amount") {
        Some((amount, false)) => fits_amount(amount)?,
        Some((_, true)) => return Err("a term's `amount` must not be optional".into()),
        None => return Err("a term needs an `amount` field".into()),
    }
    match fields.get("unit") {
        None | Some((TreeType::Str, _)) => Ok(()),
        Some((other, _)) => Err(format!("a unit must be text, got {other:?}")),
    }
}

fn fits_amount(t: &TreeType) -> Result<(), String> {
    match t {
        TreeType::Num | TreeType::Str => Ok(()),
        TreeType::Union(cases) => cases.iter().try_for_each(fits_amount),
        TreeType::Rec(f)
            if f.len() == 2
                && f.get("from") == Some(&(TreeType::Num, false))
                && f.get("to") == Some(&(TreeType::Num, false)) =>
        {
            Ok(())
        }
        other => Err(format!(
            "an amount must be a number, a {{from, to}} range of numbers, or text; got {other:?}"
        )),
    }
}

/// Total on trees of a format that passed [`fits_quantity`].
fn quantity_of(t: &Tree) -> Option<Quantity> {
    match t {
        Tree::Null => None,
        Tree::Str(s) => Some(Quantity::one(Amount::Text(s.clone()), None)),
        Tree::Rec(_) => Quantity::from_terms(vec![term_of(t)]),
        Tree::List(items) => Quantity::from_terms(items.iter().map(term_of).collect()),
        Tree::Num(_) => unreachable!("a format with a bare top-level number is refused at load"),
    }
}

fn term_of(t: &Tree) -> Term {
    let Tree::Rec(f) = t else {
        unreachable!("a term is a record, checked at load: {t:?}")
    };
    let amount = match &f["amount"] {
        Tree::Num(r) => Amount::Exact(*r),
        Tree::Str(s) => Amount::Text(s.clone()),
        Tree::Rec(r) => match (&r["from"], &r["to"]) {
            (Tree::Num(a), Tree::Num(b)) => Amount::Range(*a, *b),
            other => unreachable!("a range holds two numbers, checked at load: {other:?}"),
        },
        other => unreachable!("amount shape is checked at load: {other:?}"),
    };
    let unit = match f.get("unit") {
        None | Some(Tree::Null) => None,
        Some(Tree::Str(u)) => Some(u.clone()),
        Some(other) => unreachable!("a unit is text, checked at load: {other:?}"),
    };
    Term { amount, unit }
}

/// Candidate trees in preference order; the format's shape decides which one
/// prints.
fn trees_of(q: Option<&Quantity>) -> Vec<Tree> {
    let Some(q) = q else {
        return vec![Tree::Null];
    };
    let rec = |t: &Term| {
        let amount = match &t.amount {
            Amount::Exact(r) => Tree::Num(*r),
            Amount::Range(a, b) => Tree::Rec(BTreeMap::from([
                ("from".into(), Tree::Num(*a)),
                ("to".into(), Tree::Num(*b)),
            ])),
            Amount::Text(s) => Tree::Str(s.clone()),
        };
        let mut f = BTreeMap::from([("amount".to_string(), amount)]);
        if let Some(u) = &t.unit {
            f.insert("unit".into(), Tree::Str(u.clone()));
        }
        Tree::Rec(f)
    };
    let mut out = vec![Tree::List(q.terms().iter().map(rec).collect())];
    if let [t] = q.terms() {
        out.push(rec(t));
        if let (Amount::Text(s), None) = (&t.amount, &t.unit) {
            out.push(Tree::Str(s.clone()));
        }
    }
    out
}
