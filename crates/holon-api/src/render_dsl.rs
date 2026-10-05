//! Render DSL: the Rhai-syntax language a render source is written in.
//!
//! The source is parsed by Rhai but never evaluated: [`RenderAst`] maps the
//! syntax tree onto [`RenderExpr`], so an expression over `col(...)` becomes a
//! node the frontend evaluates per row. The grammar is closed; a form with no
//! `RenderExpr` node is refused when the source is loaded.
//!
//! Any `name(...)` call is a widget or value-function call, so a new widget
//! needs only a builder in `crates/holon-frontend/src/shadow_builders/`.
//!
//! ```rhai
//! columns(#{gap: 4, item_template: live_block()})
//!
//! tree(#{parent_id: col("parent_id"), sortkey: col("id"), item_template: render_entity()})
//!
//! row(text(col("name")), text(if col("done") { "✓" } else { "" }))
//!
//! text(`${col("quantity") * 2} ${col("unit")}`)
//! ```

use std::collections::BTreeMap;
use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use rhai::Dynamic;
use rhai::Engine as RhaiEngine;
use rhai::Expr;
use rhai::FnCallExpr;
use rhai::Position;
use rhai::Stmt;

use crate::Value;
use crate::render_types::Arg;
use crate::render_types::BinaryOperator;
use crate::render_types::RenderExpr;

/// Parse a render DSL string into a RenderExpr. A JSON-serialized
/// `RenderExpr` is accepted as well.
pub fn parse_render_dsl(source: &str) -> Result<RenderExpr> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Ok(default_table());
    }

    // Try JSON first (backwards compat)
    if let Ok(expr) = serde_json::from_str::<RenderExpr>(trimmed) {
        validate_dotted_names(&expr)?;
        validate_colour_args(&expr)?;
        return Ok(expr);
    }

    let ast = compile_expression(trimmed).context("render DSL")?;
    let expr = RenderAst
        .block(ast.statements())
        .with_context(|| format!("render DSL '{trimmed}'"))?;
    validate_colour_args(&expr)?;
    Ok(expr)
}

/// A JSON call name with a dot must be the dotted form of an alias, or the
/// expression could not be printed back as Rhai source.
fn validate_dotted_names(expr: &RenderExpr) -> Result<()> {
    if let RenderExpr::FunctionCall { name, .. } = expr {
        anyhow::ensure!(
            !name.contains('.') || DOTTED_ALIASES.iter().any(|(_, d)| d == name),
            "call name `{name}` has a dot but no Rhai call-name alias in DOTTED_ALIASES"
        );
    }
    expr.children()
        .into_iter()
        .try_for_each(validate_dotted_names)
}

/// Parse render-DSL predicate text (`eq("completed", 1)`,
/// `and(…, not(is_not_null("scheduled")))`) into a typed [`Predicate`].
///
/// The predicate constructors are render DSL calls, so this reuses the same
/// `parse → eval → serde` round-trip the `rules:` argument takes
/// (`render_interpreter::parse_rules_arg`). Fails loud with the offending
/// source when the text is not a well-formed predicate — the filter boundary
/// (FLT-1.b) turns a malformed body into a boundary error, never a default.
pub fn parse_predicate(source: &str) -> Result<crate::predicate::Predicate> {
    let expr = parse_render_dsl(source)
        .with_context(|| format!("filter predicate is not valid DSL: {source:?}"))?;
    let val = crate::eval_to_value(&expr, &HashMap::<String, crate::Value>::new())
        .with_context(|| format!("filter predicate does not evaluate: {source:?}"))?;
    let json = serde_json::to_value(&val)
        .with_context(|| format!("filter predicate Value → JSON failed: {source:?}"))?;
    serde_json::from_value::<crate::predicate::Predicate>(json)
        .with_context(|| format!("filter predicate is not a Predicate: {source:?}"))
}

/// Refuse a colour the theme does not define, for an expression that did not
/// come through the parser.
///
/// `parse_render_dsl` runs this on every doc it loads. A `RenderExpr` that was
/// deserialized from a prop (`render_expr`, `tmpl_*`) bypasses that path, so a
/// producer of such a prop must call this itself.
pub fn validate_render_expr(expr: &RenderExpr) -> Result<()> {
    validate_colour_args(expr)
}

/// Argument names whose value names a theme colour.
const COLOUR_ARGS: &[&str] = &["color", "accent"];

/// Refuse a colour the theme does not define, when the doc is LOADED.
///
/// The check lives here rather than only in the widget builders because the
/// builders are not the only way a widget gets its props: for the props-only
/// widgets (`text`, `icon`, ...) a collection's item template takes the
/// `resolve_props` fast path, which re-derives props from THIS expression
/// without running the builder at all. Validating the expression at parse time
/// covers both paths, and it is the same moment a bad `live_query(source: ...)`
/// is refused.
///
/// Only literal names are checked. A non-literal (`color: col("x")`) names no
/// colour yet, so there is nothing to refuse here; the value is resolved per
/// row and checked where it is consumed.
fn validate_colour_args(expr: &RenderExpr) -> Result<()> {
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            for arg in args {
                let arg_name = arg.name.as_deref();
                let names_a_colour = arg_name.is_some_and(|n| COLOUR_ARGS.contains(&n));
                let literal = match &arg.value {
                    RenderExpr::Literal {
                        value: Value::String(raw),
                    } => Some(raw.as_str()),
                    _ => None,
                };
                if let (true, Some(arg_name), Some(raw)) = (names_a_colour, arg_name, literal) {
                    crate::theme_token::ThemeToken::parse(raw)
                        .map_err(|e| anyhow::anyhow!("{name}(#{{{arg_name}: {raw:?}}}): {e}"))?;
                }
                validate_colour_args(&arg.value)?;
            }
            Ok(())
        }
        RenderExpr::BinaryOp { left, right, .. } => {
            validate_colour_args(left)?;
            validate_colour_args(right)
        }
        RenderExpr::Not { operand } => validate_colour_args(operand),
        RenderExpr::If {
            condition,
            then,
            otherwise,
        } => {
            validate_colour_args(condition)?;
            validate_colour_args(then)?;
            validate_colour_args(otherwise)
        }
        RenderExpr::Array { items } => {
            for item in items {
                validate_colour_args(item)?;
            }
            Ok(())
        }
        RenderExpr::Object { fields } => {
            for value in fields.values() {
                validate_colour_args(value)?;
            }
            Ok(())
        }
        RenderExpr::LiveBlock { .. }
        | RenderExpr::ColumnRef { .. }
        | RenderExpr::Literal { .. } => Ok(()),
    }
}

fn default_table() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "table".to_string(),
        args: Vec::new(),
    }
}

/// Compile a Rhai expression without evaluating it or folding constants.
pub(crate) fn compile_expression(source: &str) -> Result<rhai::AST> {
    let mut engine = RhaiEngine::new();
    engine.set_optimization_level(rhai::OptimizationLevel::None);
    engine
        .compile_expression(source)
        .map_err(|e| anyhow::anyhow!("Failed to parse '{source}': {e}"))
}

/// The `RenderExpr` of one Rhai expression, under the render DSL's closed
/// grammar.
pub(crate) fn render_expr_of(expr: &Expr) -> Result<RenderExpr> {
    RenderAst.expr(expr)
}

fn constant_to_render_expr(d: &Dynamic) -> Result<RenderExpr> {
    if d.is_map() {
        let fields = d
            .clone()
            .cast::<rhai::Map>()
            .iter()
            .map(|(k, v)| Ok((k.to_string(), constant_to_render_expr(v)?)))
            .collect::<Result<_>>()?;
        Ok(RenderExpr::Object { fields })
    } else if d.is_array() {
        let items = d
            .clone()
            .cast::<rhai::Array>()
            .iter()
            .map(constant_to_render_expr)
            .collect::<Result<_>>()?;
        Ok(RenderExpr::Array { items })
    } else {
        Ok(literal(constant_to_value(d)?))
    }
}

fn constant_to_value(d: &Dynamic) -> Result<Value> {
    if let Ok(i) = d.as_int() {
        Ok(Value::Integer(i))
    } else if let Ok(float) = d.as_float() {
        float_value(float)
    } else if let Ok(b) = d.as_bool() {
        Ok(Value::Boolean(b))
    } else if d.is_string() {
        Ok(Value::String(d.clone().into_string().expect("is_string")))
    } else if d.is_unit() {
        Ok(Value::Null)
    } else {
        anyhow::bail!(
            "render literal {d:?} of type {} has no Value form",
            d.type_name()
        )
    }
}

/// A render expression is stored and shipped as JSON, which has no infinity
/// or NaN: `serde_json` writes `null` for them and reports success.
fn float_value(float: f64) -> Result<Value> {
    anyhow::ensure!(
        float.is_finite(),
        "render literal {float} is not a finite number, so it cannot survive the JSON \
         form a render expression is stored in"
    );
    Ok(Value::Float(float))
}

// ---------------------------------------------------------------------------
// Rhai syntax tree → RenderExpr
// ---------------------------------------------------------------------------

/// Names whose calls build a `Predicate` (the serde shape `rules:` and
/// `parse_predicate` read) rather than a widget call.
const BINARY_PREDICATES: &[&str] = &["eq", "ne", "gt", "lt", "gte", "lte"];

/// Rhai has no dots in function names; these spell a dotted operation name.
const DOTTED_ALIASES: &[(&str, &str)] = &[
    ("navigation_focus", "navigation.focus"),
    ("focus_pin", "navigation.focus_pin"),
    ("navigation_close", "navigation.close"),
    ("navigation_activate", "navigation.activate"),
    ("navigation_open_tab", "navigation.open_tab"),
    (
        "integration_open_default_view",
        "integration.open_default_view",
    ),
    ("answer_question", "pending_question.answer_question"),
    // `live_session`, not `session`: the provider addresses a send by the
    // background job id, which exists only on the live registry's rows.
    ("send_message", "live_session.send_message"),
];

/// The call name that parses back to the operation `dotted`.
pub(crate) fn rhai_call_name(dotted: &str) -> &str {
    let name = DOTTED_ALIASES
        .iter()
        .find(|(_, d)| *d == dotted)
        .map_or(dotted, |(rhai, _)| rhai);
    assert!(
        !name.contains('.'),
        "operation `{dotted}` has no Rhai call-name alias in DOTTED_ALIASES"
    );
    name
}

/// Maps the syntax tree of a render source onto `RenderExpr`. The grammar is
/// closed: a form with no node here would have to be evaluated once at load,
/// so it is refused instead.
struct RenderAst;

fn refuse(form: &str, pos: Position) -> anyhow::Error {
    anyhow::anyhow!(
        "{form} at {pos} cannot be evaluated per row. The render DSL has widget and value calls, \
         col(..), literals, arrays, maps, + - * /, == != < <= > >=, && || !, `${{..}}` templates \
         and if/else"
    )
}

fn literal(value: Value) -> RenderExpr {
    RenderExpr::Literal { value }
}

fn string_literal(s: &str) -> RenderExpr {
    literal(Value::String(s.to_string()))
}

impl RenderAst {
    fn block(&self, stmts: &[Stmt]) -> Result<RenderExpr> {
        match stmts {
            [] => Ok(literal(Value::Null)),
            [stmt] => self.stmt(stmt),
            [_, second, ..] => Err(refuse("a second statement", second.position())),
        }
    }

    fn stmt(&self, stmt: &Stmt) -> Result<RenderExpr> {
        let form = match stmt {
            Stmt::Expr(expr) => return self.expr(expr),
            Stmt::FnCall(call, pos) => return self.call(call, *pos),
            Stmt::Block(block) => return self.block(block.statements()),
            Stmt::Noop(_) => return Ok(literal(Value::Null)),
            Stmt::If(flow, _) => {
                return fold(RenderExpr::If {
                    condition: Box::new(self.expr(&flow.expr)?),
                    then: Box::new(self.block(flow.body.statements())?),
                    otherwise: Box::new(self.block(flow.branch.statements())?),
                });
            }
            Stmt::Var(..) => "`let`/`const`",
            Stmt::While(..) | Stmt::Do(..) | Stmt::For(..) => "a loop",
            Stmt::Switch(..) => "`switch`",
            Stmt::Assignment(..) => "an assignment",
            Stmt::TryCatch(..) => "`try`",
            Stmt::BreakLoop(..) => "`break`/`continue`",
            Stmt::Return(..) => "`return`/`throw`",
            Stmt::Import(..) | Stmt::Export(..) => "`import`/`export`",
            Stmt::Share(..) => "a closure",
            _ => "an unknown statement",
        };
        Err(refuse(form, stmt.position()))
    }

    fn expr(&self, expr: &Expr) -> Result<RenderExpr> {
        let form = match expr {
            Expr::IntegerConstant(i, _) => return Ok(literal(Value::Integer(*i))),
            Expr::FloatConstant(f, _) => return Ok(literal(float_value(**f)?)),
            Expr::BoolConstant(b, _) => return Ok(literal(Value::Boolean(*b))),
            Expr::StringConstant(s, _) => return Ok(string_literal(s)),
            Expr::CharConstant(c, _) => return Ok(literal(Value::String(c.to_string()))),
            Expr::Unit(_) => return Ok(literal(Value::Null)),
            Expr::DynamicConstant(d, _) => return constant_to_render_expr(d),
            Expr::InterpolatedString(parts, _) => return self.template(parts),
            Expr::Array(items, _) => {
                return Ok(RenderExpr::Array {
                    items: items.iter().map(|i| self.expr(i)).collect::<Result<_>>()?,
                });
            }
            Expr::Map(map, _) => {
                return Ok(RenderExpr::Object {
                    fields: map
                        .0
                        .iter()
                        .map(|(key, value)| Ok((key.name.to_string(), self.expr(value)?)))
                        .collect::<Result<_>>()?,
                });
            }
            Expr::FnCall(call, pos) => return self.call(call, *pos),
            Expr::Stmt(block) => return self.block(block.statements()),
            Expr::And(operands, _) => return self.logical(BinaryOperator::And, operands),
            Expr::Or(operands, _) => return self.logical(BinaryOperator::Or, operands),
            Expr::Variable(..) | Expr::ThisPtr(..) => "a variable",
            Expr::Property(..) | Expr::Dot(..) | Expr::MethodCall(..) => {
                "a method call or property access"
            }
            Expr::Index(..) => "an index `[..]`",
            Expr::Coalesce(..) => "`??`",
            Expr::Custom(..) => "custom syntax",
            _ => "an unknown expression",
        };
        Err(refuse(form, expr.position()))
    }

    fn logical(&self, op: BinaryOperator, operands: &[Expr]) -> Result<RenderExpr> {
        let mut operands = operands.iter().map(|o| self.expr(o));
        let first = operands
            .next()
            .expect("Rhai builds `&&`/`||` from two operands")?;
        operands.try_fold(first, |left, right| {
            fold(RenderExpr::BinaryOp {
                op: op.clone(),
                left: Box::new(left),
                right: Box::new(right?),
            })
        })
    }

    /// A template is the concatenation of its parts, so a part that is
    /// missing for a row makes the whole text missing (D62.a).
    fn template(&self, parts: &[Expr]) -> Result<RenderExpr> {
        let mut parts = parts.iter().map(|p| self.expr(p));
        let Some(first) = parts.next() else {
            return Ok(string_literal(""));
        };
        let concat = |left, right| {
            fold(RenderExpr::BinaryOp {
                op: BinaryOperator::Concat,
                left: Box::new(left),
                right: Box::new(right),
            })
        };
        let Some(second) = parts.next() else {
            return concat(first?, string_literal(""));
        };
        parts.try_fold(concat(first?, second?)?, |left, right| concat(left, right?))
    }

    fn call(&self, call: &FnCallExpr, pos: Position) -> Result<RenderExpr> {
        if !call.namespace.is_empty() {
            return Err(refuse("a namespace-qualified call", pos));
        }
        if call.capture_parent_scope {
            return Err(refuse("a `!` scope-capturing call", pos));
        }
        let name = call.name.as_str();
        let args = call
            .args
            .iter()
            .map(|a| self.expr(a))
            .collect::<Result<Vec<_>>>()?;
        if !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            return operator(name, args, pos);
        }
        Ok(match (name, args.as_slice()) {
            (
                "col",
                [
                    RenderExpr::Literal {
                        value: Value::String(column),
                    },
                ],
            ) => RenderExpr::ColumnRef {
                name: column.clone(),
            },
            ("col", _) => return Err(refuse("`col` without one string literal", pos)),
            (
                p,
                [
                    field @ RenderExpr::Literal {
                        value: Value::String(_),
                    },
                    value,
                ],
            ) if BINARY_PREDICATES.contains(&p) => predicate(
                p,
                RenderExpr::Object {
                    fields: BTreeMap::from([
                        ("field".to_string(), field.clone()),
                        ("value".to_string(), value.clone()),
                    ]),
                },
            ),
            (
                "is_not_null",
                [
                    field @ RenderExpr::Literal {
                        value: Value::String(_),
                    },
                ],
            ) => predicate(name, field.clone()),
            ("and" | "or", [_, ..]) => predicate(name, RenderExpr::Array { items: args }),
            ("not", [inner]) => predicate(name, inner.clone()),
            ("always", []) => string_literal("always"),
            _ => {
                let name = DOTTED_ALIASES
                    .iter()
                    .find(|(rhai, _)| *rhai == name)
                    .map_or(name, |(_, dotted)| dotted);
                function_call(name, args)
            }
        })
    }
}

fn predicate(key: &str, payload: RenderExpr) -> RenderExpr {
    RenderExpr::Object {
        fields: BTreeMap::from([(key.to_string(), payload)]),
    }
}

/// A widget or value-function call. A map argument holds named arguments
/// (merged, the later key winning); every other argument is positional.
fn function_call(name: &str, args: Vec<RenderExpr>) -> RenderExpr {
    let mut named = BTreeMap::new();
    let mut positional = Vec::new();
    for arg in args {
        match arg {
            RenderExpr::Object { fields } => named.extend(fields),
            other => positional.push(other),
        }
    }
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args: named
            .into_iter()
            .map(|(k, value)| Arg {
                name: Some(k),
                value,
            })
            .chain(
                positional
                    .into_iter()
                    .map(|value| Arg { name: None, value }),
            )
            .collect(),
    }
}

/// `+` concatenates when an operand is text by syntax, as in the typed
/// computations' subset parser; otherwise it is arithmetic.
fn operator(name: &str, args: Vec<RenderExpr>, pos: Position) -> Result<RenderExpr> {
    let is_text = |e: &RenderExpr| {
        matches!(
            e,
            RenderExpr::Literal {
                value: Value::String(_)
            } | RenderExpr::BinaryOp {
                op: BinaryOperator::Concat,
                ..
            }
        )
    };
    let mut args = args.into_iter();
    let (Some(first), second, None) = (args.next(), args.next(), args.next()) else {
        return Err(refuse(&format!("operator `{name}`"), pos));
    };
    let Some(second) = second else {
        return fold(match name {
            "!" => RenderExpr::Not {
                operand: Box::new(first),
            },
            "-" => RenderExpr::BinaryOp {
                op: BinaryOperator::Sub,
                left: Box::new(literal(Value::Integer(0))),
                right: Box::new(first),
            },
            _ => return Err(refuse(&format!("unary operator `{name}`"), pos)),
        });
    };
    let op = match name {
        "+" if is_text(&first) || is_text(&second) => BinaryOperator::Concat,
        "+" => BinaryOperator::Add,
        "-" => BinaryOperator::Sub,
        "*" => BinaryOperator::Mul,
        "/" => BinaryOperator::Div,
        "==" => BinaryOperator::Eq,
        "!=" => BinaryOperator::Neq,
        "<" => BinaryOperator::Lt,
        "<=" => BinaryOperator::Lte,
        ">" => BinaryOperator::Gt,
        ">=" => BinaryOperator::Gte,
        _ => return Err(refuse(&format!("operator `{name}`"), pos)),
    };
    fold(RenderExpr::BinaryOp {
        op,
        left: Box::new(first),
        right: Box::new(second),
    })
}

/// An operator or `if` over literals only is decided now, with the same
/// evaluation a row would get.
fn fold(expr: RenderExpr) -> Result<RenderExpr> {
    let is_literal = |e: &RenderExpr| matches!(e, RenderExpr::Literal { .. });
    let constant = match &expr {
        RenderExpr::BinaryOp { left, right, .. } => is_literal(left) && is_literal(right),
        RenderExpr::Not { operand } => is_literal(operand),
        RenderExpr::If {
            condition,
            then,
            otherwise,
        } => {
            if let RenderExpr::Literal { value } = condition.as_ref() {
                return Ok(crate::render_eval::choose_branch(value, then, otherwise)?.clone());
            }
            false
        }
        _ => false,
    };
    if !constant {
        return Ok(expr);
    }
    let value = crate::render_eval::eval_to_value(&expr, &HashMap::<String, Value>::new())
        .with_context(|| format!("evaluating the constant `{}`", expr.to_rhai()))?;
    Ok(literal(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<RenderExpr> {
        parse_render_dsl(source)
    }

    /// A non-finite literal has no JSON form — `serde_json` writes it as
    /// `null` and reports success, so a node prop would carry a different
    /// expression than the author wrote.
    #[test]
    fn a_non_finite_float_literal_is_refused() {
        for source in ["text(1e400)", "text(-1e400)", "text(1e309 * 1.0)"] {
            let err = parse(source)
                .map(|expr| format!("{expr:?}"))
                .expect_err(&format!("{source} must not parse into a render expression"));
            assert!(
                format!("{err:#}").contains("finite"),
                "{source}: the refusal must name the problem; got {err:#}"
            );
        }
    }

    fn col(name: &str) -> Box<RenderExpr> {
        Box::new(RenderExpr::ColumnRef {
            name: name.to_string(),
        })
    }

    fn lit(value: Value) -> Box<RenderExpr> {
        Box::new(literal(value))
    }

    #[test]
    fn an_operator_over_a_column_is_a_per_row_node() {
        assert_eq!(
            parse(r#"col("q") * 2"#).unwrap(),
            RenderExpr::BinaryOp {
                op: BinaryOperator::Mul,
                left: col("q"),
                right: lit(Value::Integer(2)),
            }
        );
        assert_eq!(
            parse(r#"col("q") + " g""#).unwrap(),
            RenderExpr::BinaryOp {
                op: BinaryOperator::Concat,
                left: col("q"),
                right: lit(Value::String(" g".into())),
            }
        );
        assert_eq!(
            parse(r#"!col("done")"#).unwrap(),
            RenderExpr::Not {
                operand: col("done")
            }
        );
        assert_eq!(
            parse(r#"if col("done") { "x" } else { "" }"#).unwrap(),
            RenderExpr::If {
                condition: col("done"),
                then: lit(Value::String("x".into())),
                otherwise: lit(Value::String(String::new())),
            }
        );
    }

    #[test]
    fn a_template_concatenates_its_parts() {
        assert_eq!(
            parse(r#"`${col("q")} g`"#).unwrap(),
            RenderExpr::BinaryOp {
                op: BinaryOperator::Concat,
                left: col("q"),
                right: lit(Value::String(" g".into())),
            }
        );
    }

    #[test]
    fn printing_a_parsed_source_is_a_fixed_point() {
        for source in [
            r#"text(`${col("q")} ${col("unit")}`)"#,
            r#"text(`${col("q")}`)"#,
            r#"text(`${col("a")}${col("b")}${col("c")}`)"#,
            "text(2 * 3.5)",
            "text(1.0e300)",
            r#"text("a \"quoted\" \\ line\n")"#,
            r#"navigation_focus(#{region: "main", block_id: col("id")})"#,
            r#"text(col("a\"b"))"#,
            r#"text(col("a\\b"))"#,
            r#"text(live_block("a\"b"))"#,
            r#"text("x", #{"a b": "c"})"#,
            r#"text([#{"a-b": 1}])"#,
            r#"text([#{"if": 1}])"#,
            r#"text([#{alpha: 1, beta: 2, gamma: 3, delta: 4, epsilon: 5, zeta: #{y: 1, x: 2}}])"#,
        ] {
            let first = parse(source).unwrap();
            let second = parse(&first.to_rhai())
                .unwrap_or_else(|e| panic!("{source}: printed {:?}: {e:#}", first.to_rhai()));
            assert_eq!(second, first, "{source}");
            assert_eq!(second.to_rhai(), first.to_rhai(), "{source}");
        }
    }

    #[test]
    fn a_json_call_with_an_unaliased_dotted_name_is_refused_by_name() {
        for name in ["block.set_field", "whatever.thing"] {
            let json = format!(r#"{{"FunctionCall":{{"name":"{name}","args":[]}}}}"#);
            let err = format!("{:#}", parse(&json).expect_err(name));
            assert!(
                err.contains(name) && err.contains("DOTTED_ALIASES"),
                "{name}: {err}"
            );
        }
        let aliased = parse(r#"{"FunctionCall":{"name":"navigation.focus","args":[]}}"#).unwrap();
        assert_eq!(aliased.to_rhai(), "navigation_focus()");
    }

    #[test]
    fn an_operator_over_literals_is_decided_at_load() {
        assert_eq!(parse("text(2 * 3 + 1)").unwrap().to_rhai(), "text(7)");
        assert_eq!(parse("text(2 * 3.5)").unwrap().to_rhai(), "text(7.0)");
        assert_eq!(
            parse(r#"text(if 1 < 2 { "a" } else { "b" })"#)
                .unwrap()
                .to_rhai(),
            r#"text("a")"#
        );
        assert!(
            parse("text(1 / 0)").is_err(),
            "a constant error is a load error"
        );
    }

    #[test]
    fn a_form_with_no_per_row_node_is_refused_by_name() {
        for (source, form) in [
            ("text(x)", "a variable"),
            (r#"text(col("q").len())"#, "a method call"),
            (r#"text(col("q")[0])"#, "an index"),
            (r#"text(col("q") ?? "x")"#, "`??`"),
            (r#"text(col("q") % 2)"#, "operator `%`"),
            (r#"text(col(col("q")))"#, "`col` without one string literal"),
        ] {
            let err = format!("{:#}", parse(source).expect_err(source));
            assert!(err.contains(form), "{source}: expected {form:?} in {err}");
        }
    }

    #[test]
    fn a_call_takes_any_number_of_arguments() {
        let source = r#"row(icon("a"), spacer(6), text("b"), spacer(8), badge("c"), spacer(4), text("d"), text("e"))"#;
        let RenderExpr::FunctionCall { args, .. } = parse(source).unwrap() else {
            panic!("{source} must parse into a call");
        };
        assert_eq!(args.len(), 8);
    }

    #[test]
    fn test_simple_function() {
        let expr = parse("table()").unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
    }

    #[test]
    fn test_live_block() {
        let expr = parse("live_block()").unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "live_block"));
    }

    #[test]
    fn test_columns_with_named_args() {
        let expr = parse("columns(#{gap: 4, item_template: live_block()})").unwrap();
        if let RenderExpr::FunctionCall { name, args, .. } = &expr {
            assert_eq!(name, "columns");
            assert!(args.iter().any(|a| a.name.as_deref() == Some("gap")));
            assert!(
                args.iter()
                    .any(|a| a.name.as_deref() == Some("item_template"))
            );
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_col_reference() {
        let expr = parse("text(col(\"name\"))").unwrap();
        if let RenderExpr::FunctionCall { args, .. } = &expr {
            assert!(matches!(&args[0].value, RenderExpr::ColumnRef { name } if name == "name"));
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_nested_functions() {
        let expr =
            parse_render_dsl(r#"row(icon("folder"), spacer(6), text(col("name")))"#).unwrap();
        if let RenderExpr::FunctionCall { name, args, .. } = &expr {
            assert_eq!(name, "row");
            assert_eq!(args.len(), 3);
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_tree_with_col_refs() {
        let expr = parse(r#"tree(#{parent_id: col("parent_id"), sortkey: col("id"), item_template: render_entity()})"#).unwrap();
        if let RenderExpr::FunctionCall { name, args, .. } = &expr {
            assert_eq!(name, "tree");
            let parent_arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some("parent_id"))
                .unwrap();
            assert!(
                matches!(&parent_arg.value, RenderExpr::ColumnRef { name } if name == "parent_id")
            );
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_json_backwards_compat() {
        // Old JSON with operations field should still parse (serde ignores unknown
        // fields)
        let json = r#"{"FunctionCall":{"name":"table","args":[],"operations":[]}}"#;
        let expr = parse(json).unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));

        let json = r#"{"FunctionCall":{"name":"table","args":[]}}"#;
        let expr = parse(json).unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
    }

    #[test]
    fn test_empty_defaults_to_table() {
        let expr = parse("").unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
    }

    #[test]
    fn test_widget_with_integer_arg() {
        let expr = parse("chain_ops(0)").unwrap();
        if let RenderExpr::FunctionCall { name, args, .. } = &expr {
            assert_eq!(name, "chain_ops");
            assert_eq!(args.len(), 1);
        } else {
            panic!("Expected FunctionCall");
        }
    }

    /// A new widget name parses without an enum, parser, or registry change.
    #[test]
    fn new_widget_parses_without_explicit_registration() {
        let expr = parse_render_dsl(
            r#"kanban(#{group_by: "task_state", item_template: render_entity()})"#,
        )
        .unwrap();
        match expr {
            RenderExpr::FunctionCall { name, args } => {
                assert_eq!(name, "kanban");
                assert!(args.iter().any(|a| a.name.as_deref() == Some("group_by")));
                assert!(
                    args.iter()
                        .any(|a| a.name.as_deref() == Some("item_template"))
                );
            }
            other => panic!("Expected FunctionCall(kanban), got {other:?}"),
        }
    }

    #[test]
    fn nested_new_widgets_parse_without_registration() {
        let expr = parse_render_dsl(
            r#"calendar(#{view: "month", item_template: holiday_badge(text(col("title")))})"#,
        )
        .unwrap();
        if let RenderExpr::FunctionCall { name, args } = &expr {
            assert_eq!(name, "calendar");
            // The nested holiday_badge widget should have parsed too.
            let item_template = args
                .iter()
                .find(|a| a.name.as_deref() == Some("item_template"))
                .expect("calendar has item_template");
            assert!(matches!(
                &item_template.value,
                RenderExpr::FunctionCall { name, .. } if name == "holiday_badge"
            ));
        } else {
            panic!("Expected FunctionCall");
        }
    }

    // ── Predicate constructors ─────────────────────────────────
    //
    // Each constructor's output `RenderExpr` is evaluated to a `Value`, then
    // round-tripped through serde_json into a `Predicate` — the same path
    // `parse_rules_arg` (render_interpreter.rs) takes for `rules: [...]`.

    fn parse_predicate(source: &str) -> crate::predicate::Predicate {
        super::parse_predicate(source).expect("parse predicate DSL")
    }

    #[test]
    fn predicate_eq() {
        match parse_predicate(r#"eq("level", 0)"#) {
            crate::predicate::Predicate::Eq { field, value } => {
                assert_eq!(field, "level");
                assert_eq!(value, crate::Value::Integer(0));
            }
            other => panic!("expected Eq, got {other:?}"),
        }
    }

    #[test]
    fn predicate_ne_gt_lt_gte_lte() {
        // All five share a shape; spot-check each variant rather than
        // five copies of the same matching boilerplate.
        use crate::predicate::Predicate::*;
        assert!(matches!(
            parse_predicate(r#"ne("status", "done")"#),
            Ne { .. }
        ));
        assert!(matches!(parse_predicate(r#"gt("count", 3)"#), Gt { .. }));
        assert!(matches!(parse_predicate(r#"lt("count", 3)"#), Lt { .. }));
        assert!(matches!(parse_predicate(r#"gte("count", 3)"#), Gte { .. }));
        assert!(matches!(parse_predicate(r#"lte("count", 3)"#), Lte { .. }));
    }

    #[test]
    fn predicate_is_not_null() {
        use crate::predicate::Predicate::*;
        match parse_predicate(r#"is_not_null("task_state")"#) {
            IsNotNull(field) => assert_eq!(field, "task_state"),
            other => panic!("expected IsNotNull, got {other:?}"),
        }
    }

    #[test]
    fn predicate_var_via_quoted_verbose_form() {
        // `var` is a Rhai reserved keyword (the tokenizer rejects it
        // even inside map literals as an unquoted identifier), so we
        // don't register a constructor for it. Verify the quoted-key
        // verbose form `#{"var": "<field>"}` still round-trips into
        // `Predicate::Var`.
        use crate::predicate::Predicate::*;
        match parse_predicate(r#"#{"var": "is_focused"}"#) {
            Var(field) => assert_eq!(field, "is_focused"),
            other => panic!("expected Var, got {other:?}"),
        }
    }

    #[test]
    fn predicate_not() {
        use crate::predicate::Predicate::*;
        match parse_predicate(r#"not(eq("level", 0))"#) {
            Not(inner) => match *inner {
                Eq { field, .. } => assert_eq!(field, "level"),
                other => panic!("expected Not(Eq), got Not({other:?})"),
            },
            other => panic!("expected Not, got {other:?}"),
        }
    }

    #[test]
    fn predicate_and_or_variadic() {
        use crate::predicate::Predicate::*;
        match parse_predicate(r#"and(eq("level", 0), gt("depth", 0))"#) {
            And(children) => {
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], Eq { .. }));
                assert!(matches!(&children[1], Gt { .. }));
            }
            other => panic!("expected And, got {other:?}"),
        }
        // Three-arity to confirm the additional overloads are wired.
        match parse_predicate(r#"or(eq("a", 1), eq("b", 2), eq("c", 3))"#) {
            Or(children) => assert_eq!(children.len(), 3),
            other => panic!("expected Or, got {other:?}"),
        }
    }

    #[test]
    fn predicate_always() {
        assert!(matches!(
            parse_predicate("always()"),
            crate::predicate::Predicate::Always
        ));
    }

    #[test]
    fn rules_arg_round_trip_through_full_tree_call() {
        // Mirror the path `parse_rules_arg` takes on a real `tree(rules: [...])`
        // call: parse the DSL, find the `rules:` arg, evaluate it as a Value,
        // and deserialize each item as a `RuleSpec`. Confirms the constructors
        // compose with the rest of the DSL.
        let expr = parse(
            r#"tree(#{
                parent_id: col("parent_id"),
                rules: [
                    #{
                        when: and(eq("level", 0), gt("depth", 0)),
                        override: #{role: "page_title", show_bullet: false}
                    },
                    #{when: always(), override: #{}}
                ]
            })"#,
        )
        .unwrap();

        let RenderExpr::FunctionCall { args, .. } = expr else {
            panic!("expected tree(...) FunctionCall");
        };
        let rules_arg = args
            .iter()
            .find(|a| a.name.as_deref() == Some("rules"))
            .expect("rules: arg present");
        let rules_value =
            crate::eval_to_value(&rules_arg.value, &HashMap::<String, crate::Value>::new())
                .unwrap();
        let crate::Value::Array(items) = rules_value else {
            panic!("rules: must evaluate to Array");
        };

        let specs: Vec<crate::render_types::RuleSpec> = items
            .iter()
            .map(|item| {
                let json = serde_json::to_value(item).expect("Value → JSON");
                serde_json::from_value(json).expect("JSON → RuleSpec")
            })
            .collect();

        assert_eq!(specs.len(), 2);

        use crate::predicate::Predicate::*;
        match &specs[0].when {
            And(children) => {
                assert!(matches!(&children[0], Eq { field, .. } if field == "level"));
                assert!(matches!(&children[1], Gt { field, .. } if field == "depth"));
            }
            other => panic!("expected And in first rule, got {other:?}"),
        }
        assert_eq!(
            specs[0].overrides.get("role").and_then(|v| v.as_string()),
            Some("page_title")
        );
        assert_eq!(
            specs[0].overrides.get("show_bullet"),
            Some(&crate::Value::Boolean(false))
        );

        assert!(matches!(specs[1].when, Always));
    }
}
