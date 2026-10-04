use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Null,
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn to_rhai_dynamic(&self) -> rhai::Dynamic {
        match self {
            Value::Float(f) => rhai::Dynamic::from(*f),
            Value::Int(i) => rhai::Dynamic::from(*i),
            Value::String(s) => rhai::Dynamic::from(s.clone()),
            Value::Bool(b) => rhai::Dynamic::from(*b),
            Value::Null => rhai::Dynamic::UNIT,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Float(v) => write!(f, "{v}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::String(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Null => write!(f, "null"),
        }
    }
}

impl Value {
    /// Refuses non-finite floats: they have no faithful YAML/JSON form.
    pub fn try_from_dynamic(d: rhai::Dynamic) -> Result<Self, String> {
        if d.is_unit() {
            Ok(Value::Null)
        } else if let Ok(b) = d.as_bool() {
            Ok(Value::Bool(b))
        } else if let Ok(i) = d.as_int() {
            Ok(Value::Int(i))
        } else if let Ok(f) = d.as_float() {
            if f.is_finite() {
                Ok(Value::Float(f))
            } else {
                Err(format!("non-finite float {f}"))
            }
        } else if let Ok(s) = d.into_string() {
            Ok(Value::String(s))
        } else {
            Ok(Value::Null)
        }
    }
}
