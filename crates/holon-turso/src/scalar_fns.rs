//! The Rust scalar functions that every Holon database registers when it opens.
//!
//! A materialized view that calls one of them loads only when the database was
//! opened with it, and Turso refuses a second in-process open with a different
//! set. So every open path takes the set from [`ALL`]: the core open in
//! [`crate::turso::TursoBackend::open_database`] and the SDK `Builder` users
//! through [`register_on_builder`].

use holon_api::Value;
use holon_api::computation::ArithOp;
use holon_api::computation::arith_apply;
use turso_core::DeterministicScalarFn;
use turso_core::Numeric;
use turso_core::OpenOptions;

/// One registered function. `version` is part of the database identity: bump
/// it when the function computes a different result for the same arguments,
/// because an incremental view retracts old rows by calling the new code.
#[derive(Clone, Copy)]
pub struct ScalarFn {
    pub name: &'static str,
    pub arg_count: usize,
    pub version: u32,
    pub func: DeterministicScalarFn,
}

pub const ALL: &[ScalarFn] = &[
    arith(ArithOp::Add, holon_add),
    arith(ArithOp::Sub, holon_sub),
    arith(ArithOp::Mul, holon_mul),
    arith(ArithOp::Div, holon_div),
];

const fn arith(op: ArithOp, func: DeterministicScalarFn) -> ScalarFn {
    ScalarFn {
        name: op.sql_fn(),
        arg_count: 2,
        version: 2,
        func,
    }
}

fn holon_add(args: &[turso_core::Value]) -> Result<turso_core::Value, String> {
    checked(ArithOp::Add, args)
}

fn holon_sub(args: &[turso_core::Value]) -> Result<turso_core::Value, String> {
    checked(ArithOp::Sub, args)
}

fn holon_mul(args: &[turso_core::Value]) -> Result<turso_core::Value, String> {
    checked(ArithOp::Mul, args)
}

fn holon_div(args: &[turso_core::Value]) -> Result<turso_core::Value, String> {
    checked(ArithOp::Div, args)
}

/// [`arith_apply`] over SQL values. The engine checks the argument count
/// against the registration before it calls.
fn checked(op: ArithOp, args: &[turso_core::Value]) -> Result<turso_core::Value, String> {
    let [lhs, rhs] = args else {
        return Err(format!("{} takes 2 arguments, got {args:?}", op.sql_fn()));
    };
    match arith_apply(op, &operand(op, lhs)?, &operand(op, rhs)?).map_err(|e| e.to_string())? {
        Value::Integer(i) => Ok(turso_core::Value::from_i64(i)),
        Value::Float(f) => Ok(turso_core::Value::from_f64(f)),
        Value::Null => Ok(turso_core::Value::Null),
        other => Err(format!("{} computed the non-number {other:?}", op.sql_fn())),
    }
}

fn operand(op: ArithOp, v: &turso_core::Value) -> Result<Value, String> {
    Ok(match v {
        turso_core::Value::Null => Value::Null,
        turso_core::Value::Numeric(Numeric::Integer(i)) => Value::Integer(*i),
        turso_core::Value::Numeric(Numeric::Float(f)) => Value::Float((*f).into()),
        turso_core::Value::Text(t) => Value::String(t.as_str().to_string()),
        turso_core::Value::Blob(b) => {
            return Err(format!(
                "{} refuses a {}-byte BLOB operand: arithmetic takes numbers",
                op.sql_fn(),
                b.len()
            ));
        }
    })
}

/// The identity of a function set, independent of declaration order. A
/// database records the signature it was built with.
pub fn signature(fns: &[ScalarFn]) -> String {
    let mut parts: Vec<String> = fns
        .iter()
        .map(|f| format!("{}/{}/v{}", f.name, f.arg_count, f.version))
        .collect();
    parts.sort();
    parts.join(",")
}

pub(crate) fn register_on_options(options: OpenOptions, fns: &[ScalarFn]) -> OpenOptions {
    fns.iter().fold(options, |options, f| {
        options.deterministic_scalar_function(f.name, f.arg_count, f.func)
    })
}

pub(crate) fn sdk_functions() -> Vec<turso_sdk_kit::rsapi::DeterministicScalarFunction> {
    ALL.iter()
        .map(|f| turso_sdk_kit::rsapi::DeterministicScalarFunction {
            name: f.name.to_string(),
            arg_count: f.arg_count,
            func: f.func,
        })
        .collect()
}

pub fn register_on_builder(builder: turso::Builder) -> turso::Builder {
    ALL.iter().fold(builder, |builder, f| {
        builder.with_deterministic_scalar_function(f.name, f.arg_count, f.func)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(args: &[turso_core::Value]) -> Result<turso_core::Value, String> {
        Ok(args[0].clone())
    }

    #[test]
    fn signature_ignores_declaration_order_and_names_the_version() {
        let a = ScalarFn {
            name: "a_fn",
            arg_count: 1,
            version: 2,
            func: identity,
        };
        let b = ScalarFn {
            name: "b_fn",
            arg_count: 3,
            version: 1,
            func: identity,
        };
        assert_eq!(signature(&[b, a]), "a_fn/1/v2,b_fn/3/v1");
        assert_eq!(signature(&[a, b]), signature(&[b, a]));
        assert_eq!(signature(&[]), "");
    }
}
