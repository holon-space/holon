use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use holon_expr::CompiledExpr;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Ge,
    Le,
    Eq,
    Ne,
    Gt,
    Lt,
}

impl CmpOp {
    /// Ordered longest-prefix-first so `>=` is tried before `>`.
    pub const ALL: [CmpOp; 6] = [
        CmpOp::Ge,
        CmpOp::Le,
        CmpOp::Eq,
        CmpOp::Ne,
        CmpOp::Gt,
        CmpOp::Lt,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CmpOp::Ge => ">=",
            CmpOp::Le => "<=",
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Gt => ">",
            CmpOp::Lt => "<",
        }
    }
}

/// A precondition spec on an input arc, parsed once at net load.
///
/// Yaml surface syntax (the value side of `precond: {attr: <spec>}`):
/// - `"$name"`          — placeholder bind (captures the attribute value)
/// - `">= 0.2"` etc.    — comparison of the attribute against a Rhai expression
///   (operators: `==`, `!=`, `>=`, `<=`, `>`, `<`), compiled at load
/// - anything else      — exact literal match
///
/// Anything that starts like an operator (`=`, `<`, `>`, `!`) but is not one of
/// the valid operators is rejected at load — it would otherwise silently become
/// an exact match that never fires (e.g. `"= done"` or `"=> 5"`).
#[derive(Clone)]
pub enum PrecondSpec {
    /// Full spec including the `$` prefix — used verbatim as the placeholder
    /// map key.
    Placeholder(String),
    Comparison {
        op: CmpOp,
        rhs: String,
        /// `x <op> <rhs>` compiled once at load.
        compiled: CompiledExpr,
    },
    Exact(String),
}

impl FromStr for PrecondSpec {
    type Err = String;

    fn from_str(spec: &str) -> Result<Self, String> {
        if let Some(name) = spec.strip_prefix('$') {
            if name.is_empty() {
                return Err(format!(
                    "invalid precondition spec '{spec}': '$' must be followed by a placeholder \
                     name"
                ));
            }
            return Ok(PrecondSpec::Placeholder(spec.to_string()));
        }
        for op in CmpOp::ALL {
            if let Some(rhs) = spec.strip_prefix(op.as_str()) {
                let rhs = rhs.trim().to_string();
                if rhs.is_empty() {
                    return Err(format!(
                        "invalid precondition spec '{spec}': operator '{}' has no right-hand side",
                        op.as_str()
                    ));
                }
                let engine = rhai::Engine::new();
                let compiled = CompiledExpr::compile(&engine, format!("x {} {}", op.as_str(), rhs))
                    .map_err(|e| {
                        format!(
                            "invalid precondition spec '{spec}': right-hand side does not \
                             compile: {e}"
                        )
                    })?;
                return Ok(PrecondSpec::Comparison { op, rhs, compiled });
            }
        }
        if spec.starts_with(['=', '<', '>', '!']) {
            let bad_op: String = spec
                .chars()
                .take_while(|c| matches!(c, '=' | '<' | '>' | '!'))
                .collect();
            return Err(format!(
                "invalid precondition spec '{spec}': '{bad_op}' is not a valid operator; valid \
                 operators are ==, !=, >=, <=, >, < (e.g. '== done'), '$name' for a placeholder, \
                 or a bare literal for exact match"
            ));
        }
        Ok(PrecondSpec::Exact(spec.to_string()))
    }
}

impl fmt::Display for PrecondSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrecondSpec::Placeholder(s) | PrecondSpec::Exact(s) => f.write_str(s),
            PrecondSpec::Comparison { op, rhs, .. } => write!(f, "{} {rhs}", op.as_str()),
        }
    }
}

impl fmt::Debug for PrecondSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrecondSpec::Placeholder(s) => f.debug_tuple("Placeholder").field(s).finish(),
            PrecondSpec::Comparison { op, rhs, .. } => f
                .debug_struct("Comparison")
                .field("op", op)
                .field("rhs", rhs)
                .finish(),
            PrecondSpec::Exact(s) => f.debug_tuple("Exact").field(s).finish(),
        }
    }
}

impl PartialEq for PrecondSpec {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PrecondSpec::Placeholder(a), PrecondSpec::Placeholder(b)) => a == b,
            (PrecondSpec::Exact(a), PrecondSpec::Exact(b)) => a == b,
            (
                PrecondSpec::Comparison { op: a, rhs: ra, .. },
                PrecondSpec::Comparison { op: b, rhs: rb, .. },
            ) => a == b && ra == rb,
            _ => false,
        }
    }
}

impl Serialize for PrecondSpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PrecondSpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputArc {
    pub bind: String,
    pub token_type: String,
    #[serde(default)]
    pub precond: BTreeMap<String, PrecondSpec>,
    #[serde(default)]
    pub consume: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputArc {
    pub from: String,
    #[serde(default)]
    pub postcond: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateArc {
    pub id_expr: String,
    pub token_type: String,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_documented_operators_parse() {
        for op in CmpOp::ALL {
            let spec: PrecondSpec = format!("{} 5", op.as_str()).parse().unwrap();
            match spec {
                PrecondSpec::Comparison {
                    op: parsed, rhs, ..
                } => {
                    assert_eq!(parsed, op);
                    assert_eq!(rhs, "5");
                }
                other => panic!("expected Comparison for '{}', got {other:?}", op.as_str()),
            }
        }
        assert_eq!(
            "$who".parse::<PrecondSpec>().unwrap(),
            PrecondSpec::Placeholder("$who".to_string())
        );
        assert_eq!(
            "active".parse::<PrecondSpec>().unwrap(),
            PrecondSpec::Exact("active".to_string())
        );
    }

    #[test]
    fn malformed_operators_fail_loudly() {
        for bad in ["= done", "=> 5", "!done", "=done", "==", ">= ", "$"] {
            let err = bad.parse::<PrecondSpec>().unwrap_err();
            assert!(
                err.contains(&format!("'{bad}'")),
                "error for {bad:?} must name the spec, got: {err}"
            );
        }
        let err = "= done".parse::<PrecondSpec>().unwrap_err();
        assert!(err.contains("=="), "error must list valid operators: {err}");
    }
}
