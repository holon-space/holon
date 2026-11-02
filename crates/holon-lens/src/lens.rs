//! Resourceful string lenses (Boomerang-style) declared in YAML.
//!
//! One declaration gives `get` (text → tree), `put` (tree + original text →
//! text, unchanged chunks byte-for-byte) and `create` (tree → text). A
//! declaration is parsed into a [`Decl`] tree first, so the shared
//! [`Alphabet`] can be built from every regex before any automaton is.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;

use regex_syntax::hir::Hir;
use serde_yaml::Value as Y;

use crate::lang::Alphabet;
use crate::lang::Dfa;
use crate::lang::concat_ambiguity;
use crate::quantity::QuantityError;
use crate::quantity::Rational;

#[derive(Clone, PartialEq, Debug)]
pub enum Tree {
    Null,
    Str(String),
    Num(Rational),
    Rec(BTreeMap<String, Tree>),
    List(Vec<Tree>),
}

/// The abstract type of what `get` returns, for the load-time binding check.
#[derive(Clone, PartialEq, Debug)]
pub enum TreeType {
    Null,
    Str,
    Num,
    /// Field name → (type, may be absent).
    Rec(BTreeMap<String, (TreeType, bool)>),
    List(Box<TreeType>),
    Union(Vec<TreeType>),
}

type R<T> = Result<T, String>;

// ---------------------------------------------------------------- declaration

struct Re {
    src: String,
    hir: Hir,
}

impl Re {
    fn parse(src: &str, path: &str) -> R<Re> {
        let hir = regex_syntax::ParserBuilder::new()
            .build()
            .parse(src)
            .map_err(|e| format!("{path}: bad regex {src:?}:\n{e}"))?;
        Ok(Re {
            src: src.to_string(),
            hir,
        })
    }
}

struct Decl {
    kind: DeclKind,
    path: String,
}

enum DeclKind {
    Copy(Re),
    Const {
        re: Re,
        default: String,
    },
    Number {
        re: Re,
        seps: String,
        print_sep: char,
        round: Option<u32>,
    },
    Fraction(Re),
    Field {
        name: String,
        inner: Box<Decl>,
    },
    Seq(Vec<Decl>),
    Union {
        cases: Vec<Decl>,
        otherwise: Option<Box<Decl>>,
    },
    List {
        item: Box<Decl>,
        sep: Option<Box<Decl>>,
        key: Option<String>,
        min: u64,
    },
    Optional(Box<Decl>),
    Except {
        inner: Box<Decl>,
        not: Re,
        reason: String,
    },
}

const LENS_KEYS: &str = "copy, const, number, fraction, seq, union, list, optional, except";

fn str_of<'a>(y: &'a Y, path: &str) -> R<&'a str> {
    y.as_str()
        .ok_or_else(|| format!("{path}: expected a string, got {}", yaml_text(y)))
}

fn yaml_text(y: &Y) -> String {
    serde_yaml::to_string(y).map_or_else(|e| format!("<{e}>"), |s| s.trim_end().to_string())
}

/// The keys of mapping `y`, refusing any outside `allowed`.
fn keys_in<'a>(y: &'a Y, allowed: &[&str], path: &str) -> R<&'a serde_yaml::Mapping> {
    let m = y
        .as_mapping()
        .ok_or_else(|| format!("{path}: expected a mapping, got {}", yaml_text(y)))?;
    for k in m.keys() {
        let k = str_of(k, path)?;
        if !allowed.contains(&k) {
            return Err(format!(
                "{path}: unknown key `{k}` (allowed: {})",
                allowed.join(", ")
            ));
        }
    }
    Ok(m)
}

fn seq_of<'a>(y: &'a Y, path: &str) -> R<&'a Vec<Y>> {
    y.as_sequence()
        .ok_or_else(|| format!("{path}: expected a list"))
}

fn decl(y: &Y, path: &str) -> R<Decl> {
    let m = y.as_mapping().ok_or_else(|| {
        format!(
            "{path}: expected a mapping like `copy: …`, got {}",
            yaml_text(y)
        )
    })?;
    if let Some(name) = m.get("field") {
        let name = str_of(name, path)?.to_string();
        let mut rest = m.clone();
        rest.remove("field");
        let p = format!("{path} > field `{name}`");
        let inner = decl(&Y::Mapping(rest), &p)?;
        return Ok(Decl {
            kind: DeclKind::Field {
                name,
                inner: Box::new(inner),
            },
            path: p,
        });
    }
    let keys: Vec<&str> = m.keys().map(|k| str_of(k, path)).collect::<R<_>>()?;
    let lens_keys: Vec<&str> = keys.iter().copied().filter(|k| *k != "otherwise").collect();
    let [key] = lens_keys.as_slice() else {
        return Err(format!(
            "{path}: expected exactly one of {LENS_KEYS}, got {keys:?}"
        ));
    };
    let otherwise = m.get("otherwise");
    if otherwise.is_some() && *key != "union" {
        return Err(format!(
            "{path}: `otherwise` belongs to a `union`, not to `{key}`"
        ));
    }
    let v = &m[*key];
    let p = format!("{path} > {key}");
    let kind = match *key {
        "copy" => DeclKind::Copy(Re::parse(str_of(v, &p)?, &p)?),
        "const" => {
            let (re, default) = match v {
                Y::String(s) => (regex_syntax::escape(s), s.clone()),
                _ => {
                    keys_in(v, &["match", "default"], &p)?;
                    (
                        str_of(&v["match"], &p)?.to_string(),
                        str_of(&v["default"], &p)?.to_string(),
                    )
                }
            };
            DeclKind::Const {
                re: Re::parse(&re, &p)?,
                default,
            }
        }
        "number" => {
            keys_in(v, &["decimal", "print_decimal", "round"], &p)?;
            let seps = v
                .get("decimal")
                .map(|d| str_of(d, &p))
                .transpose()?
                .unwrap_or(".")
                .to_string();
            let print_sep = match v.get("print_decimal") {
                Some(d) => str_of(d, &p)?.to_string(),
                None => seps.chars().take(1).collect(),
            };
            let mut chars = print_sep.chars();
            let (Some(print_sep), None) = (chars.next(), chars.next()) else {
                return Err(format!(
                    "{p}: print_decimal must be one character, got {print_sep:?}"
                ));
            };
            if seps.is_empty()
                || !seps.contains(print_sep)
                || seps.chars().any(|c| c.is_ascii_digit())
            {
                return Err(format!(
                    "{p}: decimal {seps:?} must list the separators (no digits), print_decimal \
                     {print_sep:?} among them"
                ));
            }
            let round = v
                .get("round")
                .map(|r| {
                    r.as_u64()
                        .filter(|&r| r <= 18)
                        .map(|r| r as u32)
                        .ok_or_else(|| format!("{p}: round must be a whole number from 0 to 18"))
                })
                .transpose()?;
            let class: String = seps
                .chars()
                .map(|c| regex_syntax::escape(&c.to_string()))
                .collect();
            // The language holds only values `get` can read without overflowing
            // `i128` (below 10^37); a longer number falls to the union's `otherwise`.
            DeclKind::Number {
                re: Re::parse(
                    &format!("[0-9]{{1,{INT_DIGITS}}}([{class}][0-9]{{1,18}})?"),
                    &p,
                )?,
                seps,
                print_sep,
                round,
            }
        }
        "fraction" => {
            keys_in(v, &[], &p)?;
            // Only fractions `get` can read: a non-zero denominator, and whole·den + num
            // within `i128`.
            DeclKind::Fraction(Re::parse(
                "([0-9]{1,18} +)?[0-9]{1,18} */ *0{0,17}[1-9][0-9]{0,17}",
                &p,
            )?)
        }
        "seq" => DeclKind::Seq(
            seq_of(v, &p)?
                .iter()
                .enumerate()
                .map(|(i, y)| decl(y, &format!("{p}[{i}]")))
                .collect::<R<_>>()?,
        ),
        "union" => DeclKind::Union {
            cases: seq_of(v, &p)?
                .iter()
                .enumerate()
                .map(|(i, y)| decl(y, &format!("{p}[{i}]")))
                .collect::<R<_>>()?,
            otherwise: otherwise
                .map(|o| decl(o, &format!("{p} > otherwise")).map(Box::new))
                .transpose()?,
        },
        "list" => {
            keys_in(v, &["item", "separator", "key", "min"], &p)?;
            DeclKind::List {
                item: Box::new(decl(&v["item"], &format!("{p} > item"))?),
                sep: v
                    .get("separator")
                    .map(|s| decl(s, &format!("{p} > separator")).map(Box::new))
                    .transpose()?,
                key: v
                    .get("key")
                    .map(|k| str_of(k, &p).map(str::to_string))
                    .transpose()?,
                min: v
                    .get("min")
                    .map(|n| {
                        n.as_u64()
                            .ok_or_else(|| format!("{p}: min must be a whole number"))
                    })
                    .transpose()?
                    .unwrap_or(0),
            }
        }
        "optional" => DeclKind::Optional(Box::new(decl(v, &p)?)),
        "except" => {
            keys_in(v, &["lens", "not", "reason"], &p)?;
            DeclKind::Except {
                inner: Box::new(decl(&v["lens"], &format!("{p} > lens"))?),
                not: Re::parse(str_of(&v["not"], &p)?, &p)?,
                reason: str_of(&v["reason"], &p)?.to_string(),
            }
        }
        other => {
            return Err(format!(
                "{path}: unknown lens `{other}` (expected one of {LENS_KEYS})"
            ));
        }
    };
    Ok(Decl { kind, path: p })
}

impl Decl {
    fn regexes<'a>(&'a self, out: &mut Vec<&'a Hir>) {
        match &self.kind {
            DeclKind::Copy(re)
            | DeclKind::Fraction(re)
            | DeclKind::Const { re, .. }
            | DeclKind::Number { re, .. } => out.push(&re.hir),
            DeclKind::Field { inner, .. } | DeclKind::Optional(inner) => inner.regexes(out),
            DeclKind::Seq(parts) => parts.iter().for_each(|p| p.regexes(out)),
            DeclKind::Union { cases, otherwise } => cases
                .iter()
                .chain(otherwise.as_deref())
                .for_each(|p| p.regexes(out)),
            DeclKind::List { item, sep, .. } => {
                item.regexes(out);
                if let Some(s) = sep {
                    s.regexes(out);
                }
            }
            DeclKind::Except { inner, not, .. } => {
                inner.regexes(out);
                out.push(&not.hir);
            }
        }
    }
}

// ---------------------------------------------------------------- compiled
// lens

/// What a lens contributes to the abstract side.
#[derive(Clone, Debug)]
enum Shape {
    Unit,
    Fields(BTreeSet<String>),
    Value,
}

#[derive(Debug)]
enum Abs {
    Unit,
    Fields(BTreeMap<String, Tree>),
    Value(Tree),
}

enum Kind {
    Copy,
    Const {
        default: String,
    },
    Number {
        seps: String,
        print_sep: char,
        round: Option<u32>,
    },
    Fraction,
    Field {
        name: String,
        inner: Box<Node>,
    },
    Seq {
        parts: Vec<Node>,
        suffix: Vec<Dfa>,
    },
    Union(Vec<Node>),
    List {
        item: Box<Node>,
        sep: Option<Box<Node>>,
        key: Option<String>,
        min: u64,
        rest: Dfa,
        item_rest: Dfa,
    },
    Optional(Box<Node>),
    Except {
        inner: Box<Node>,
        not: Dfa,
        reason: String,
    },
}

pub struct Node {
    kind: Kind,
    pub lang: Dfa,
    shape: Shape,
    path: String,
}

pub fn compile(y: &Y) -> R<Node> {
    let d = decl(y, "lens")?;
    let mut regexes = Vec::new();
    d.regexes(&mut regexes);
    let alpha = Arc::new(Alphabet::of(&regexes));
    node(&d, &alpha)
}

fn regex_dfa(alpha: &Arc<Alphabet>, re: &Re, path: &str) -> R<Dfa> {
    Dfa::regex(alpha, &re.hir).map_err(|e| format!("{path}: regex {:?}: {e}", re.src))
}

fn node(d: &Decl, alpha: &Arc<Alphabet>) -> R<Node> {
    let p = d.path.clone();
    let made = |kind, lang, shape| {
        Ok(Node {
            kind,
            lang,
            shape,
            path: p.clone(),
        })
    };
    match &d.kind {
        DeclKind::Copy(re) => made(Kind::Copy, regex_dfa(alpha, re, &p)?, Shape::Value),
        DeclKind::Const { re, default } => {
            let lang = regex_dfa(alpha, re, &p)?;
            if !lang.accepts(default) {
                return Err(format!(
                    "{p}: default {default:?} does not match {:?}",
                    re.src
                ));
            }
            made(
                Kind::Const {
                    default: default.clone(),
                },
                lang,
                Shape::Unit,
            )
        }
        DeclKind::Number {
            re,
            seps,
            print_sep,
            round,
        } => made(
            Kind::Number {
                seps: seps.clone(),
                print_sep: *print_sep,
                round: *round,
            },
            regex_dfa(alpha, re, &p)?,
            Shape::Value,
        ),
        DeclKind::Fraction(re) => made(Kind::Fraction, regex_dfa(alpha, re, &p)?, Shape::Value),
        DeclKind::Field { name, inner } => {
            let inner = node(inner, alpha)?;
            if matches!(inner.shape, Shape::Unit) {
                return Err(format!(
                    "{p}: a field must record something, but this lens only matches constant text"
                ));
            }
            Ok(Node {
                lang: inner.lang.clone(),
                kind: Kind::Field {
                    name: name.clone(),
                    inner: Box::new(inner),
                },
                shape: Shape::Fields(BTreeSet::from([name.clone()])),
                path: p.clone(),
            })
        }
        DeclKind::Seq(parts) => {
            let parts: Vec<Node> = parts.iter().map(|d| node(d, alpha)).collect::<R<_>>()?;
            let mut suffix = vec![Dfa::epsilon(alpha); parts.len() + 1];
            for i in (0..parts.len()).rev() {
                suffix[i] = Dfa::concat(alpha, &[&parts[i].lang, &suffix[i + 1]]);
            }
            for i in 0..parts.len().saturating_sub(1) {
                if let Some(w) = concat_ambiguity(&parts[i].lang, &suffix[i + 1]) {
                    return Err(format!(
                        "{p}: ambiguous — the text {:?} can be split two ways between part {i} ({}) \
                         and what follows it:\n    {:?} | {:?}\n    {:?} | {:?}\n  make part {i} \
                         unable to end the way the next part can start (or vice versa)",
                        w.text,
                        parts[i].path,
                        &w.text[..w.split_a],
                        &w.text[w.split_a..],
                        &w.text[..w.split_b],
                        &w.text[w.split_b..]
                    ));
                }
            }
            let shape = seq_shape(&parts, &p)?;
            made(
                Kind::Seq {
                    parts,
                    suffix: suffix.clone(),
                },
                suffix[0].clone(),
                shape,
            )
        }
        DeclKind::Union { cases, otherwise } => {
            let mut parts: Vec<Node> = cases.iter().map(|d| node(d, alpha)).collect::<R<_>>()?;
            for i in 0..parts.len() {
                for j in i + 1..parts.len() {
                    if let Some(w) = parts[i].lang.intersect(&parts[j].lang).example() {
                        return Err(format!(
                            "{p}: ambiguous — the text {w:?} matches both case {i} ({}) and case \
                             {j} ({}); cases must not overlap (use `except` or `otherwise`)",
                            parts[i].path, parts[j].path
                        ));
                    }
                }
            }
            if let Some(o) = otherwise {
                let other = node(o, alpha)?;
                let taken = Dfa::union(alpha, &parts.iter().map(|n| &n.lang).collect::<Vec<_>>());
                let shape = other.shape.clone();
                parts.push(Node {
                    lang: other.lang.minus(&taken),
                    kind: Kind::Except {
                        inner: Box::new(other),
                        not: taken,
                        reason: "the text matches an earlier case, so `otherwise` cannot write it"
                            .into(),
                    },
                    shape,
                    path: o.path.clone(),
                });
            }
            let lang = Dfa::union(alpha, &parts.iter().map(|n| &n.lang).collect::<Vec<_>>());
            made(Kind::Union(parts), lang, Shape::Value)
        }
        DeclKind::List {
            item,
            sep,
            key,
            min,
        } => {
            let item = Box::new(node(item, alpha)?);
            let sep = sep
                .as_deref()
                .map(|s| node(s, alpha))
                .transpose()?
                .map(Box::new);
            if let Some(s) = &sep
                && !matches!(s.shape, Shape::Unit)
            {
                return Err(format!(
                    "{p}: the separator must be constant text (`const`)"
                ));
            }
            if let Some(k) = key
                && !matches!(&item.shape, Shape::Fields(f) if f.contains(k))
            {
                return Err(format!("{p}: key `{k}` is not a field of the item"));
            }
            let elem = match &sep {
                Some(s) => Dfa::concat(alpha, &[&s.lang, &item.lang]),
                None => item.lang.clone(),
            };
            if elem.accepts_empty() {
                return Err(format!(
                    "{p}: an item can be empty, so a list of them can be split in endless ways"
                ));
            }
            let rest = elem.star();
            for (a, b, what) in [
                (&elem, &rest, "two neighbouring items"),
                (&item.lang, &rest, "the first item and the rest"),
            ] {
                if let Some(w) = concat_ambiguity(a, b) {
                    return Err(format!(
                        "{p}: ambiguous — the text {:?} can be cut into items two ways ({what}):\n    \
                         {:?} | {:?}\n    {:?} | {:?}",
                        w.text,
                        &w.text[..w.split_a],
                        &w.text[w.split_a..],
                        &w.text[..w.split_b],
                        &w.text[w.split_b..]
                    ));
                }
            }
            let item_rest = Dfa::concat(alpha, &[&item.lang, &rest]);
            let lang = if *min == 0 {
                item_rest.optional()
            } else {
                item_rest.clone()
            };
            if *min > 1 {
                return Err(format!("{p}: min may be 0 or 1"));
            }
            made(
                Kind::List {
                    item,
                    sep,
                    key: key.clone(),
                    min: *min,
                    rest,
                    item_rest,
                },
                lang,
                Shape::Value,
            )
        }
        DeclKind::Optional(inner) => {
            let inner = node(inner, alpha)?;
            if inner.lang.accepts_empty() {
                return Err(format!(
                    "{p}: the inner lens already matches empty text, so `optional` would be ambiguous"
                ));
            }
            Ok(Node {
                lang: inner.lang.optional(),
                shape: inner.shape.clone(),
                kind: Kind::Optional(Box::new(inner)),
                path: p.clone(),
            })
        }
        DeclKind::Except { inner, not, reason } => {
            let inner = node(inner, alpha)?;
            let not = regex_dfa(alpha, not, &p)?;
            let shape = inner.shape.clone();
            let lang = inner.lang.minus(&not);
            made(
                Kind::Except {
                    inner: Box::new(inner),
                    not,
                    reason: reason.clone(),
                },
                lang,
                shape,
            )
        }
    }
}

fn seq_shape(parts: &[Node], path: &str) -> R<Shape> {
    let values = parts
        .iter()
        .filter(|n| matches!(n.shape, Shape::Value))
        .count();
    let mut fields = BTreeSet::new();
    for n in parts {
        if let Shape::Fields(f) = &n.shape {
            for k in f {
                if !fields.insert(k.clone()) {
                    return Err(format!("{path}: field `{k}` is declared twice"));
                }
            }
        }
    }
    match (values, fields.is_empty()) {
        (0, true) => Ok(Shape::Unit),
        (0, false) => Ok(Shape::Fields(fields)),
        (1, true) => Ok(Shape::Value),
        _ => Err(format!(
            "{path}: a sequence holds an unnamed value next to other values; name each with \
             `field:` so `put` knows which is which"
        )),
    }
}

impl Node {
    /// The type of the tree `get` returns for this lens.
    pub fn tree_type(&self) -> TreeType {
        match &self.shape {
            Shape::Unit => TreeType::Null,
            Shape::Fields(_) => TreeType::Rec(self.fields_type()),
            Shape::Value => self.value_type(),
        }
    }

    fn fields_type(&self) -> BTreeMap<String, (TreeType, bool)> {
        match &self.kind {
            Kind::Field { name, inner } => {
                BTreeMap::from([(name.clone(), (inner.tree_type(), false))])
            }
            Kind::Seq { parts, .. } => parts
                .iter()
                .filter(|p| matches!(p.shape, Shape::Fields(_)))
                .flat_map(|p| p.fields_type())
                .collect(),
            Kind::Optional(inner) => inner
                .fields_type()
                .into_iter()
                .map(|(k, (t, _))| (k, (t, true)))
                .collect(),
            Kind::Except { inner, .. } => inner.fields_type(),
            _ => unreachable!(
                "{}: only fields, sequences, optionals and excepts carry fields",
                self.path
            ),
        }
    }

    fn value_type(&self) -> TreeType {
        match &self.kind {
            Kind::Copy => TreeType::Str,
            Kind::Number { .. } | Kind::Fraction => TreeType::Num,
            Kind::Seq { parts, .. } => parts
                .iter()
                .find(|p| matches!(p.shape, Shape::Value))
                .expect("a value-shaped sequence has one value part")
                .value_type(),
            Kind::Union(cases) => TreeType::Union(cases.iter().map(Node::tree_type).collect()),
            Kind::List { item, .. } => TreeType::List(Box::new(item.tree_type())),
            Kind::Optional(inner) => TreeType::Union(vec![inner.value_type(), TreeType::Null]),
            Kind::Except { inner, .. } => inner.value_type(),
            Kind::Const { .. } | Kind::Field { .. } => {
                unreachable!("{}: constants and fields carry no bare value", self.path)
            }
        }
    }
}

// ---------------------------------------------------------------- get

/// Why one candidate print failed. `refused` marks a declared exclusion
/// (`except` with its `reason`), as opposed to a value that does not fit.
#[derive(Clone, Debug)]
pub struct Fail {
    pub path: String,
    pub msg: String,
    pub refused: bool,
}

type P<T> = Result<T, Vec<Fail>>;

fn fail(path: &str, msg: impl Into<String>) -> Vec<Fail> {
    vec![Fail {
        path: path.to_string(),
        msg: msg.into(),
        refused: false,
    }]
}

pub fn get(top: &Node, s: &str) -> R<Tree> {
    if !top.lang.accepts(s) {
        let (at, expected) = top.lang.mismatch(s);
        let seen: String = s.chars().take(at).collect();
        let rest: String = s.chars().skip(at).collect();
        let where_ = if rest.is_empty() {
            format!("it ends too early after {seen:?}")
        } else {
            format!("it goes wrong at {rest:?} (after {seen:?})")
        };
        return Err(format!(
            "{s:?} does not match the format: {where_}; expected {expected}"
        ));
    }
    abs_get(top, s).map(to_tree)
}

fn to_tree(a: Abs) -> Tree {
    match a {
        Abs::Unit => Tree::Null,
        Abs::Fields(f) => Tree::Rec(f),
        Abs::Value(v) => v,
    }
}

/// End of the unique prefix of `s` in `head` whose remainder is in `tail`.
fn split(head: &Dfa, tail: &Dfa, s: &str) -> usize {
    let mut st = 0;
    if head.is_accepting(st) && tail.accepts(s) {
        return 0;
    }
    for (i, c) in s.char_indices() {
        match head.step(st, c) {
            Some(n) => st = n,
            None => break,
        }
        let end = i + c.len_utf8();
        if head.is_accepting(st) && tail.accepts(&s[end..]) {
            return end;
        }
    }
    unreachable!("split called on text outside head·tail")
}

fn seq_chunks<'a>(parts: &[Node], suffix: &[Dfa], s: &'a str) -> Vec<&'a str> {
    let mut out = vec![];
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        let end = pos + split(&part.lang, &suffix[i + 1], &s[pos..]);
        out.push(&s[pos..end]);
        pos = end;
    }
    out
}

/// Items and separators of a list text: (items, separators between them).
fn list_chunks<'a>(
    item: &Node,
    sep: Option<&Node>,
    rest: &Dfa,
    item_rest: &Dfa,
    s: &'a str,
) -> (Vec<&'a str>, Vec<&'a str>) {
    let (mut items, mut seps) = (vec![], vec![]);
    if s.is_empty() {
        return (items, seps);
    }
    let mut pos = 0;
    loop {
        let end = pos + split(&item.lang, rest, &s[pos..]);
        items.push(&s[pos..end]);
        pos = end;
        if pos == s.len() {
            return (items, seps);
        }
        if let Some(sep) = sep {
            let e = pos + split(&sep.lang, item_rest, &s[pos..]);
            seps.push(&s[pos..e]);
            pos = e;
        }
    }
}

fn abs_get(n: &Node, s: &str) -> R<Abs> {
    let at = |e: QuantityError| format!("{}: {e}", n.path);
    Ok(match &n.kind {
        Kind::Copy => Abs::Value(Tree::Str(s.to_string())),
        Kind::Const { .. } => Abs::Unit,
        Kind::Number { seps, .. } => {
            Abs::Value(Tree::Num(Rational::parse_decimal(s, seps).map_err(at)?))
        }
        Kind::Fraction => Abs::Value(Tree::Num(parse_fraction(s).map_err(at)?)),
        Kind::Field { name, inner } => Abs::Fields(BTreeMap::from([(
            name.clone(),
            to_tree(abs_get(inner, s)?),
        )])),
        Kind::Seq { parts, suffix } => {
            let mut fields = BTreeMap::new();
            let mut value = None;
            for (p, c) in parts.iter().zip(seq_chunks(parts, suffix, s)) {
                match abs_get(p, c)? {
                    Abs::Unit => {}
                    Abs::Fields(f) => fields.extend(f),
                    Abs::Value(v) => value = Some(v),
                }
            }
            match &n.shape {
                Shape::Unit => Abs::Unit,
                Shape::Fields(_) => Abs::Fields(fields),
                Shape::Value => {
                    Abs::Value(value.expect("a value-shaped sequence has one value part"))
                }
            }
        }
        Kind::Union(cases) => {
            let case = cases
                .iter()
                .find(|c| c.lang.accepts(s))
                .expect("union text matches a case");
            Abs::Value(to_tree(abs_get(case, s)?))
        }
        Kind::List {
            item,
            sep,
            rest,
            item_rest,
            ..
        } => {
            let (items, _) = list_chunks(item, sep.as_deref(), rest, item_rest, s);
            Abs::Value(Tree::List(
                items
                    .iter()
                    .map(|c| abs_get(item, c).map(to_tree))
                    .collect::<R<_>>()?,
            ))
        }
        Kind::Optional(inner) if s.is_empty() => match inner.shape {
            Shape::Value => Abs::Value(Tree::Null),
            Shape::Fields(_) => Abs::Fields(BTreeMap::new()),
            Shape::Unit => Abs::Unit,
        },
        Kind::Optional(inner) | Kind::Except { inner, .. } => abs_get(inner, s)?,
    })
}

/// `"1/2"`, `"1 1/2"`; the lens's regex has already checked the form.
fn parse_fraction(s: &str) -> Result<Rational, QuantityError> {
    let (whole, frac) = match s.split_once('/') {
        Some((head, _)) if head.trim_end().contains(' ') => {
            let (w, rest) = s.split_once(' ').expect("checked above");
            (w, rest.trim_start())
        }
        _ => ("0", s),
    };
    let (a, b) = frac.split_once('/').expect("a fraction has a slash");
    let int = |t: &str| {
        t.trim()
            .parse::<i128>()
            .map_err(|_| QuantityError::Overflow(format!("{s:?}")))
    };
    Rational::int(int(whole)?).checked_add(Rational::new(int(a)?, int(b)?)?)
}

// ---------------------------------------------------------------- put / create

/// `put` with `orig = Some(text)`, `create` with `None`. `orig` must be in
/// the lens's language.
pub fn put(top: &Node, v: &Tree, orig: Option<&str>) -> P<String> {
    if let Some(o) = orig {
        assert!(
            top.lang.accepts(o),
            "put: the original {o:?} is outside the format"
        );
    }
    let out = abs_put(top, &from_tree(top, v)?, orig)?;
    debug_assert!(top.lang.accepts(&out), "printed {out:?} outside the format");
    Ok(out)
}

fn from_tree(n: &Node, v: &Tree) -> P<Abs> {
    Ok(match (&n.shape, v) {
        (Shape::Unit, Tree::Null) => Abs::Unit,
        (Shape::Fields(_), Tree::Rec(f)) => Abs::Fields(f.clone()),
        (Shape::Value, v) => Abs::Value(v.clone()),
        (s, v) => {
            return Err(fail(
                &n.path,
                format!("value {v:?} does not fit here ({s:?} expected)"),
            ));
        }
    })
}

fn abs_put(n: &Node, a: &Abs, orig: Option<&str>) -> P<String> {
    let mismatch = || fail(&n.path, format!("value {a:?} does not fit here"));
    let quantity = |e: QuantityError| fail(&n.path, e.to_string());
    let out = match (&n.kind, a) {
        (Kind::Copy, Abs::Value(Tree::Str(s))) => s.clone(),
        (Kind::Const { default }, Abs::Unit) => orig.unwrap_or(default).to_string(),
        (
            Kind::Number {
                seps,
                print_sep,
                round,
            },
            Abs::Value(Tree::Num(r)),
        ) => match orig {
            Some(o) if Rational::parse_decimal(o, seps).map_err(quantity)? == *r => o.to_string(),
            _ => {
                let sep = orig
                    .and_then(|o| o.chars().find(|c| seps.contains(*c)))
                    .unwrap_or(*print_sep);
                print_decimal(*r, sep, *round).map_err(|e| fail(&n.path, e))?
            }
        },
        (Kind::Fraction, Abs::Value(Tree::Num(r))) => match orig {
            Some(o) if parse_fraction(o).map_err(quantity)? == *r => o.to_string(),
            _ => {
                let (w, rem) = (r.num() / r.den(), r.num() % r.den());
                match (w, rem) {
                    (_, 0) => {
                        return Err(fail(
                            &n.path,
                            format!("{r} is whole, it has no fraction form"),
                        ));
                    }
                    (0, _) => format!("{rem}/{}", r.den()),
                    _ => format!("{w} {}/{}", rem.abs(), r.den()),
                }
            }
        },
        (Kind::Field { name, inner }, Abs::Fields(f)) => {
            let v = f
                .get(name)
                .ok_or_else(|| fail(&n.path, format!("the value has no field `{name}`")))?;
            abs_put(inner, &from_tree(inner, v)?, orig)?
        }
        (Kind::Seq { parts, suffix }, a) => {
            let chunks: Vec<Option<&str>> = match orig {
                Some(o) => seq_chunks(parts, suffix, o).into_iter().map(Some).collect(),
                None => vec![None; parts.len()],
            };
            if let (Abs::Fields(f), Shape::Fields(known)) = (a, &n.shape)
                && let Some(k) = f.keys().find(|k| !known.contains(*k))
            {
                return Err(fail(
                    &n.path,
                    format!("the value has field `{k}`, which this format cannot write"),
                ));
            }
            let mut out = String::new();
            for (p, c) in parts.iter().zip(chunks) {
                let sub = match (&p.shape, a) {
                    (Shape::Unit, _) => Abs::Unit,
                    (Shape::Fields(keys), Abs::Fields(f)) => Abs::Fields(
                        f.iter()
                            .filter(|(k, _)| keys.contains(*k))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    ),
                    (Shape::Value, Abs::Value(v)) => Abs::Value(v.clone()),
                    _ => return Err(mismatch()),
                };
                out += &abs_put(p, &sub, c)?;
            }
            out
        }
        (Kind::Union(cases), Abs::Value(v)) => {
            let mut fails = vec![];
            if let Some(o) = orig {
                let case = cases
                    .iter()
                    .find(|c| c.lang.accepts(o))
                    .expect("union text matches a case");
                match from_tree(case, v).and_then(|a| abs_put(case, &a, Some(o))) {
                    Ok(s) => return Ok(s),
                    Err(f) => fails.extend(f),
                }
            }
            for case in cases {
                match from_tree(case, v).and_then(|a| abs_put(case, &a, None)) {
                    Ok(s) => return Ok(s),
                    Err(f) => fails.extend(f),
                }
            }
            return Err(fails);
        }
        (
            Kind::List {
                item,
                sep,
                key,
                min,
                rest,
                item_rest,
            },
            Abs::Value(Tree::List(vs)),
        ) => {
            if (vs.len() as u64) < *min {
                return Err(fail(
                    &n.path,
                    format!("the list needs at least {min} item(s), got {}", vs.len()),
                ));
            }
            let (items, seps) = match orig {
                Some(o) => list_chunks(item, sep.as_deref(), rest, item_rest, o),
                None => (vec![], vec![]),
            };
            let key_of = |t: &Tree| match (key, t) {
                (Some(k), Tree::Rec(f)) => Some(f.get(k).cloned().unwrap_or(Tree::Null)),
                _ => None,
            };
            let orig_keys: Vec<Option<Tree>> = items
                .iter()
                .map(|c| {
                    abs_get(item, c)
                        .map(|a| key_of(&to_tree(a)))
                        .map_err(|e| fail(&n.path, e))
                })
                .collect::<P<_>>()?;
            let mut used = vec![false; items.len()];
            let mut out = String::new();
            for (i, v) in vs.iter().enumerate() {
                if i > 0 {
                    let s = sep
                        .as_deref()
                        .expect("a list of several items without separator is refused at load");
                    out += &abs_put(s, &Abs::Unit, seps.get(i - 1).copied())?;
                }
                let slot = match key {
                    Some(_) => {
                        let k = key_of(v);
                        (0..items.len()).find(|&j| !used[j] && orig_keys[j] == k)
                    }
                    None => (i < items.len()).then_some(i),
                };
                if let Some(j) = slot {
                    used[j] = true;
                }
                out += &abs_put(item, &from_tree(item, v)?, slot.map(|j| items[j]))?;
            }
            out
        }
        (Kind::Optional(inner), a) => {
            let absent = match a {
                Abs::Value(Tree::Null) => true,
                Abs::Fields(f) => f.is_empty(),
                Abs::Unit => orig.is_none_or(str::is_empty),
                Abs::Value(_) => false,
            };
            if absent {
                String::new()
            } else {
                abs_put(inner, a, orig.filter(|o| !o.is_empty()))?
            }
        }
        (Kind::Except { inner, not, reason }, a) => {
            let s = abs_put(inner, a, orig)?;
            if not.accepts(&s) {
                return Err(vec![Fail {
                    path: n.path.clone(),
                    msg: format!("the value would print as {s:?}, which is excluded: {reason}"),
                    refused: true,
                }]);
            }
            s
        }
        _ => return Err(mismatch()),
    };
    if !n.lang.accepts(&out) {
        return Err(fail(
            &n.path,
            format!("printed {out:?}, which does not match this part of the format"),
        ));
    }
    Ok(out)
}

/// The most integer digits the number language holds.
const INT_DIGITS: u32 = 19;

fn print_decimal(r: Rational, sep: char, round: Option<u32>) -> Result<String, String> {
    let past_bound = || format!("{r} has more than {INT_DIGITS} integer digits, the number bound");
    if (r.num() / r.den()).unsigned_abs() >= 10u128.pow(INT_DIGITS) {
        return Err(past_bound());
    }
    let (digits, k) = match round {
        Some(k) => r
            .round(k)
            .map_err(|e| e.to_string())?
            .to_decimal(k)
            .ok_or_else(past_bound)?,
        None => r.to_decimal(18).ok_or_else(|| {
            format!("{r} has no exact decimal form (declare `round:` to allow rounding)")
        })?,
    };
    let (neg, digits) = match digits.strip_prefix('-') {
        Some(d) => ("-", d.to_string()),
        None => ("", digits),
    };
    let k = k as usize;
    let digits = format!("{digits:0>width$}", width = k + 1);
    let (i, f) = digits.split_at(digits.len() - k);
    Ok(if k == 0 {
        format!("{neg}{i}")
    } else {
        format!("{neg}{i}{sep}{f}")
    })
}
