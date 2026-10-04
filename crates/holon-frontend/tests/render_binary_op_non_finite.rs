//! A render-spec arithmetic expression whose finite operands overflow to
//! +/-inf must render as an error node naming the operator and the value,
//! never as the text `inf`.

use std::sync::Arc;

use holon_api::Value;
use holon_api::render_types::BinaryOperator;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::view_model::ViewKind;

fn column(name: &str) -> Box<RenderExpr> {
    Box::new(RenderExpr::ColumnRef {
        name: name.to_string(),
    })
}

fn literal(f: f64) -> Box<RenderExpr> {
    Box::new(RenderExpr::Literal {
        value: Value::Float(f),
    })
}

#[test]
fn overflowing_binary_op_over_a_column_renders_an_error_node() {
    let cases = [
        (BinaryOperator::Mul, 1e308, 10.0, "*"),
        (BinaryOperator::Add, 1.7e308, 1.7e308, "+"),
        (BinaryOperator::Sub, -1.7e308, 1.7e308, "-"),
        (BinaryOperator::Div, 1e308, 1e-308, "/"),
    ];
    for (op, a, b, symbol) in cases {
        let mut row = DataRow::new();
        row.insert("x".to_string(), Value::Float(a));
        let expr = RenderExpr::BinaryOp {
            op,
            left: column("x"),
            right: literal(b),
        };
        let ctx = RenderContext::default().with_row(Arc::new(row));
        let vm = StubBuilderServices::new().interpret(&expr, &ctx).snapshot();
        match vm.kind {
            ViewKind::Error { message } => assert!(
                message.contains("non-finite float") && message.contains(symbol),
                "{a:?} {symbol} {b:?}: error must name the value and operator, got {message}"
            ),
            other => panic!("{a:?} {symbol} {b:?}: expected an error node, got {other:?}"),
        }
    }
}

fn overflowing_arg() -> holon_api::render_types::Arg {
    holon_api::render_types::Arg {
        name: Some("ratio".to_string()),
        value: RenderExpr::BinaryOp {
            op: BinaryOperator::Mul,
            left: column("x"),
            right: literal(10.0),
        },
    }
}

fn row_with_huge_x() -> DataRow {
    let mut row = DataRow::new();
    row.insert("x".to_string(), Value::Float(1e308));
    row
}

#[test]
fn operation_params_built_from_an_overflowing_expression_are_refused() {
    let action = RenderExpr::FunctionCall {
        name: "block.set_ratio".to_string(),
        args: vec![overflowing_arg()],
    };
    let err = holon_frontend::operations::parse_action_expr(&action, &row_with_huge_x())
        .expect_err("a non-finite param must refuse the operation");
    let message = err.to_string();
    assert!(
        message.contains("non-finite float inf") && message.contains("1e308 * 10.0"),
        "error must name the value and the expression, got {message}"
    );
}

#[test]
fn widget_args_built_from_an_overflowing_expression_render_an_error_node() {
    let expr = RenderExpr::FunctionCall {
        name: "text".to_string(),
        args: vec![overflowing_arg()],
    };
    let ctx = RenderContext::default().with_row(Arc::new(row_with_huge_x()));
    let vm = StubBuilderServices::new().interpret(&expr, &ctx).snapshot();
    match vm.kind {
        ViewKind::Error { message } => assert!(
            message.contains("non-finite float inf"),
            "error must name the value, got {message}"
        ),
        other => panic!("expected an error node, got {other:?}"),
    }
}
