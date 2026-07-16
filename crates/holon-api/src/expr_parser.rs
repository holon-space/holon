//! @c4 component
//! @c4 layer Engine
//! Pattern: Recursive-descent parser
//!
//! The **A2 subset parser** (ruling: C4 derived-fields, "A2 subset-first parser
//! on the unchanged `=` surface"). It parses a deliberate *syntactic subset* of
//! Rhai directly into a typed [`Computation`], so a derived field declared as a
//! `= …` property can be SQL-planted (seat A) instead of forced onto the Rhai
//! projection stage (seat B).
//!
//! The subset:
//!
//! ```text
//! expr        := if_expr | switch_expr | comparison
//! if_expr     := 'if' comparison block ('else' 'if' comparison block)* 'else' block
//! switch_expr := 'switch' comparison '{' arm (',' arm)* ','? '}'
//! arm         := ('-'? number) '=>' expr | '_' '=>' expr     // labels: distinct numeric literals
//! block       := '{' expr '}'
//! comparison  := additive ( ('=='|'!='|'<'|'<='|'>'|'>=') additive )?
//! additive    := multiplicative ( ('+'|'-') multiplicative )*
//! multiplicative := unary ( ('*'|'/') unary )*
//! unary       := '-'? primary
//! primary     := number | identifier | '(' expr ')'
//! ```
//!
//! `if`/`switch` both lower to [`Computation::Case`] (see its docs): `switch`
//! keeps its scrutinee; `if` uses a `true` scrutinee with each branch's
//! `match_value` being the boolean condition.
//!
//! This is a **total function that fails loud with a typed error** — it does
//! NOT swallow. The *caller* (petri `=`-prop path) treats a parse error as the
//! disclosed signal to fall back to the full Rhai compiler
//! ([`Computation::Script`]); falling back to Rhai is the deliberate,
//! disclosed design, not the parser hiding a problem.

use std::collections::HashSet;
use std::fmt;

use crate::Value;
use crate::computation::ArithOp;
use crate::computation::CmpOp;
use crate::computation::Computation;

/// A subset-parse failure. Not a user error on its own: the derived-field
/// pipeline falls back to Rhai on `Err`. Carries a message for disclosure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprParseError {
    pub message: String,
}

impl ExprParseError {
    fn new(message: impl Into<String>) -> Self {
        ExprParseError {
            message: message.into(),
        }
    }
}

impl fmt::Display for ExprParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "subset parse rejected expression: {}", self.message)
    }
}

impl std::error::Error for ExprParseError {}

/// Parse `src` (the expression AFTER the leading `=` has been stripped) as the
/// Rhai subset into a typed [`Computation`]. `Err` means "not in the subset";
/// the caller falls back to Rhai.
pub fn parse(src: &str) -> Result<Computation, ExprParseError> {
    let tokens = tokenize(src)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() {
        return Err(ExprParseError::new(format!(
            "trailing tokens after expression at position {}",
            parser.pos
        )));
    }
    Ok(expr)
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// Numeric literal: the value plus whether it was written in float form
    /// (a `.` or exponent), which decides `Value::Float` vs `Value::Integer`.
    Num {
        text: String,
        is_float: bool,
    },
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Cmp(CmpOp),
    FatArrow,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Comma,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, ExprParseError> {
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '0'..='9' => {
                let start = i;
                let mut is_float = false;
                while i < bytes.len() {
                    let d = bytes[i] as char;
                    if d.is_ascii_digit() {
                        i += 1;
                    } else if d == '.' {
                        is_float = true;
                        i += 1;
                    } else if d == 'e' || d == 'E' {
                        is_float = true;
                        i += 1;
                        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                            i += 1;
                        }
                    } else {
                        break;
                    }
                }
                out.push(Tok::Num {
                    text: src[start..i].to_string(),
                    is_float,
                });
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let start = i;
                while i < bytes.len() {
                    let d = bytes[i] as char;
                    if d.is_ascii_alphanumeric() || d == '_' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push(Tok::Ident(src[start..i].to_string()));
            }
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '*' => {
                out.push(Tok::Star);
                i += 1;
            }
            '/' => {
                out.push(Tok::Slash);
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '{' => {
                out.push(Tok::LBrace);
                i += 1;
            }
            '}' => {
                out.push(Tok::RBrace);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '<' => {
                i += 1;
                if next_is(bytes, i, b'=') {
                    out.push(Tok::Cmp(CmpOp::Le));
                    i += 1;
                } else {
                    out.push(Tok::Cmp(CmpOp::Lt));
                }
            }
            '>' => {
                i += 1;
                if next_is(bytes, i, b'=') {
                    out.push(Tok::Cmp(CmpOp::Ge));
                    i += 1;
                } else {
                    out.push(Tok::Cmp(CmpOp::Gt));
                }
            }
            '=' => {
                i += 1;
                if next_is(bytes, i, b'=') {
                    out.push(Tok::Cmp(CmpOp::Eq));
                    i += 1;
                } else if next_is(bytes, i, b'>') {
                    out.push(Tok::FatArrow);
                    i += 1;
                } else {
                    return Err(ExprParseError::new("bare `=` is not in the subset"));
                }
            }
            '!' => {
                i += 1;
                if next_is(bytes, i, b'=') {
                    out.push(Tok::Cmp(CmpOp::Ne));
                    i += 1;
                } else {
                    return Err(ExprParseError::new("`!` without `=` is not in the subset"));
                }
            }
            other => {
                return Err(ExprParseError::new(format!(
                    "unexpected character `{other}`"
                )));
            }
        }
    }
    Ok(out)
}

fn next_is(bytes: &[u8], i: usize, b: u8) -> bool {
    i < bytes.len() && bytes[i] == b
}

// ---------------------------------------------------------------------------
// Recursive-descent parser
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn peek_ident(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s == kw)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, tok: &Tok, what: &str) -> Result<(), ExprParseError> {
        match self.bump() {
            Some(ref t) if t == tok => Ok(()),
            other => Err(ExprParseError::new(format!(
                "expected {what}, found {other:?}"
            ))),
        }
    }

    fn parse_expr(&mut self) -> Result<Computation, ExprParseError> {
        if self.peek_ident("if") {
            self.parse_if()
        } else if self.peek_ident("switch") {
            self.parse_switch()
        } else {
            self.parse_comparison()
        }
    }

    fn parse_block(&mut self) -> Result<Computation, ExprParseError> {
        self.expect(&Tok::LBrace, "`{`")?;
        let inner = self.parse_expr()?;
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(inner)
    }

    /// `if c1 {r1} else if c2 {r2} else {e}` -> Case with `true` scrutinee and
    /// each branch's match_value being the boolean condition. `else` is
    /// required (a value-producing `if` in the derived-field surface always has
    /// one).
    fn parse_if(&mut self) -> Result<Computation, ExprParseError> {
        self.expect(&Tok::Ident("if".into()), "`if`")?;
        let mut branches = Vec::new();
        let cond = self.parse_comparison()?;
        let body = self.parse_block()?;
        branches.push((cond, body));
        loop {
            self.expect(&Tok::Ident("else".into()), "`else`")?;
            if self.peek_ident("if") {
                self.bump(); // consume `if`
                let cond = self.parse_comparison()?;
                let body = self.parse_block()?;
                branches.push((cond, body));
            } else {
                let else_body = self.parse_block()?;
                return Ok(Computation::Case {
                    scrutinee: Box::new(Computation::Lit(Value::Boolean(true))),
                    branches,
                    else_: Box::new(else_body),
                });
            }
        }
    }

    /// `switch x { a => r, b => r2, _ => e }` -> Case with scrutinee `x`. Case
    /// labels are **numeric literals only** (Rhai requires constant cases), and
    /// **duplicate labels are rejected** (Rhai rejects them too) — parse, don't
    /// validate. The `_` default arm is required. `Integer(2)` and `Float(2.0)`
    /// are DISTINCT labels (Rhai `switch` is type-strict), so they never
    /// collide.
    fn parse_switch(&mut self) -> Result<Computation, ExprParseError> {
        self.expect(&Tok::Ident("switch".into()), "`switch`")?;
        let scrutinee = self.parse_comparison()?;
        self.expect(&Tok::LBrace, "`{`")?;
        let mut branches = Vec::new();
        let mut else_: Option<Computation> = None;
        let mut seen_labels: HashSet<String> = HashSet::new();
        loop {
            if matches!(self.peek(), Some(Tok::RBrace)) {
                break;
            }
            if self.peek_ident("_") {
                self.bump();
                self.expect(&Tok::FatArrow, "`=>`")?;
                let result = self.parse_expr()?;
                if else_.is_some() {
                    return Err(ExprParseError::new("duplicate `_` default arm in switch"));
                }
                else_ = Some(result);
            } else {
                let label = self.parse_switch_label()?;
                if !seen_labels.insert(format!("{label:?}")) {
                    return Err(ExprParseError::new(format!(
                        "duplicate switch case label `{label:?}`"
                    )));
                }
                self.expect(&Tok::FatArrow, "`=>`")?;
                let result = self.parse_expr()?;
                branches.push((Computation::Lit(label), result));
            }
            if matches!(self.peek(), Some(Tok::Comma)) {
                self.bump();
            } else {
                break;
            }
        }
        self.expect(&Tok::RBrace, "`}`")?;
        let else_ = else_
            .ok_or_else(|| ExprParseError::new("switch without a `_` default arm is rejected"))?;
        Ok(Computation::Case {
            scrutinee: Box::new(scrutinee),
            branches,
            else_: Box::new(else_),
        })
    }

    /// A switch case label: a numeric literal with an optional leading `-`.
    fn parse_switch_label(&mut self) -> Result<Value, ExprParseError> {
        let negate = if matches!(self.peek(), Some(Tok::Minus)) {
            self.bump();
            true
        } else {
            false
        };
        match self.bump() {
            Some(Tok::Num { text, is_float }) => {
                if is_float {
                    let n: f64 = text.parse().map_err(|e| {
                        ExprParseError::new(format!("bad float case label `{text}`: {e}"))
                    })?;
                    Ok(Value::Float(if negate { -n } else { n }))
                } else {
                    let n: i64 = text.parse().map_err(|e| {
                        ExprParseError::new(format!("bad integer case label `{text}`: {e}"))
                    })?;
                    Ok(Value::Integer(if negate { -n } else { n }))
                }
            }
            other => Err(ExprParseError::new(format!(
                "switch case label must be a numeric literal, found {other:?}"
            ))),
        }
    }

    fn parse_comparison(&mut self) -> Result<Computation, ExprParseError> {
        let lhs = self.parse_additive()?;
        if let Some(Tok::Cmp(op)) = self.peek() {
            let op = *op;
            self.bump();
            let rhs = self.parse_additive()?;
            return Ok(Computation::Compare {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            });
        }
        Ok(lhs)
    }

    fn parse_additive(&mut self) -> Result<Computation, ExprParseError> {
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => ArithOp::Add,
                Some(Tok::Minus) => ArithOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_multiplicative()?;
            lhs = Computation::Arith {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> Result<Computation, ExprParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => ArithOp::Mul,
                Some(Tok::Slash) => ArithOp::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = Computation::Arith {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    /// Unary minus lowers to `Integer(0) - operand`, so no new Computation
    /// shape is needed. The `Integer(0)` LHS is type-preserving under the
    /// arithmetic rules (int − int = int, int − float = float), mirroring
    /// Rhai's negation: `-5` stays `Integer(-5)`, `-5.0` is `Float(-5.0)`.
    /// A `Float(0.0)` LHS would wrongly promote `-5` to a float.
    fn parse_unary(&mut self) -> Result<Computation, ExprParseError> {
        if matches!(self.peek(), Some(Tok::Minus)) {
            self.bump();
            let operand = self.parse_unary()?;
            return Ok(Computation::Arith {
                op: ArithOp::Sub,
                lhs: Box::new(Computation::Lit(Value::Integer(0))),
                rhs: Box::new(operand),
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Computation, ExprParseError> {
        match self.bump() {
            Some(Tok::Num { text, is_float }) => {
                let value = if is_float {
                    Value::Float(text.parse::<f64>().map_err(|e| {
                        ExprParseError::new(format!("bad float literal `{text}`: {e}"))
                    })?)
                } else {
                    Value::Integer(text.parse::<i64>().map_err(|e| {
                        ExprParseError::new(format!("bad integer literal `{text}`: {e}"))
                    })?)
                };
                Ok(Computation::Lit(value))
            }
            Some(Tok::Ident(name)) => {
                if matches!(
                    name.as_str(),
                    "if" | "else" | "switch" | "_" | "true" | "false"
                ) {
                    return Err(ExprParseError::new(format!(
                        "keyword `{name}` is not a valid primary in the subset"
                    )));
                }
                Ok(Computation::Field(name))
            }
            Some(Tok::LParen) => {
                let inner = self.parse_expr()?;
                self.expect(&Tok::RParen, "`)`")?;
                Ok(inner)
            }
            other => Err(ExprParseError::new(format!(
                "expected a number, identifier, or `(`, found {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
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
        let c = parse("switch priority { 3.0 => 100.0, 2.0 => 40.0, _ => 1.0 }").unwrap();
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
    fn rejects_non_literal_switch_case_label() {
        // Rhai requires constant case labels; `a + b` is not one.
        assert!(parse("switch x { a + b => 1.0, _ => 0.0 }").is_err());
    }

    #[test]
    fn rejects_out_of_subset_constructs() {
        // Function calls, string literals, boolean ops -> Err (caller falls back).
        assert!(parse("max(a, b)").is_err());
        assert!(parse("\"hello\"").is_err());
        assert!(parse("a && b").is_err());
        assert!(parse("let x = 1").is_err());
    }
}
