//! Render DSL: the Rhai-syntax language a render source is written in.
//!
//! The source is parsed by Rhai but never evaluated: `RenderAst` maps the
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
/// `and(…, not(is_not_null("scheduled")))`) into a typed `Predicate`.
///
/// The predicate constructors are render DSL calls, so this reuses the same
/// `parse → eval → serde` round-trip the `rules:` argument takes
/// (`render_interpreter::parse_rules_arg`). Fails loud with the offending
/// source when the text is not a well-formed predicate — the filter boundary
/// (FLT-1.b) turns a malformed body into a boundary error, never a default.
pub fn parse_predicate(source: &str) -> Result<crate::predicate::Predicate> {
    let expr = parse_render_dsl(source)
        .with_context(|| format!("filter predicate is not valid DSL: {source:?}"))?;
    let val = crate::render_eval::eval_plain_value(&expr, &HashMap::<String, crate::Value>::new())
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
    let value = crate::render_eval::eval_plain_value(&expr, &HashMap::<String, Value>::new())
        .with_context(|| format!("evaluating the constant `{}`", expr.to_rhai()))?;
    Ok(literal(value))
}

#[cfg(test)]
mod tests;
