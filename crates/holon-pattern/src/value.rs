//! The FRB-shaped dynamic `Value` and its conversions.
//!
//! Lives below `holon-api` so the guard `Pattern` AST (which embeds `Value` in
//! its literals) is reachable from crates that must not depend on `holon-api`.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

/// Value type shaped for flutter_rust_bridge interop. // ALLOW(compatibility):
/// FRB constrains the variant shape
///
/// This type is used in holon-prql-render and re-exported by holon
/// to ensure type consistency across the codebase.
///
/// flutter_rust_bridge:non_opaque
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
/// @c4 code
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    // DateTime variant: stored as RFC3339 string for flutter_rust_bridge interop. //
    // ALLOW(compatibility): FRB doesn't expose chrono::DateTime Use as_datetime() to get the
    // parsed chrono::DateTime
    DateTime(String),
    // Json variant: stored as String for flutter_rust_bridge interop. // ALLOW(compatibility):
    // FRB doesn't expose serde_json::Value Use as_json_value() to get the parsed
    // serde_json::Value
    Json(String),
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
    Null,
}

impl Value {
    /// Get the serde_json::Value if this is a Json variant
    ///
    /// flutter_rust_bridge:ignore
    pub fn as_json_value(&self) -> Option<serde_json::Value> {
        match self {
            // ALLOW(ok) ALLOW(fallback): malformed Json variant returns None (FRB-shaped string)
            Value::Json(s) => serde_json::from_str(s).ok(),
            _ => None,
        }
    }

    /// Create a Value from a serde_json::Value
    ///
    /// flutter_rust_bridge:ignore
    pub fn from_json_value(v: serde_json::Value) -> Self {
        // Try to convert to a more specific variant first
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Boolean(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Integer(i)
                } else if n.is_f64() {
                    // `is_f64` rather than `as_f64`: the latter also answers for
                    // an integer too large for `i64`, and rounding one into an
                    // `f64` loses digits silently.
                    Value::Float(n.as_f64().expect("is_f64 just answered"))
                } else {
                    Value::Json(n.to_string())
                }
            }
            serde_json::Value::String(s) => Value::String(s),
            serde_json::Value::Array(arr) => {
                Value::Array(arr.into_iter().map(Value::from_json_value).collect())
            }
            serde_json::Value::Object(obj) => Value::Object(
                obj.into_iter()
                    .map(|(k, v)| (k, Value::from_json_value(v)))
                    .collect(),
            ),
        }
    }

    /// Get string value, returning None if not a string
    ///
    /// flutter_rust_bridge:ignore
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get string value as owned String, returning None if not a string
    pub fn as_string_owned(&self) -> Option<String> {
        match self {
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            Value::Float(f) => Some(*f as i64),
            // SQLite TEXT-affinity columns store integers as strings
            // ALLOW(ok): SQLite TEXT-affinity int parse
            Value::String(s) => s.parse::<i64>().ok(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Get datetime value as RFC3339 string
    ///
    /// flutter_rust_bridge:ignore
    pub fn as_datetime_string(&self) -> Option<&str> {
        match self {
            Value::DateTime(s) => Some(s),
            _ => None,
        }
    }

    /// Get datetime value as parsed chrono::DateTime
    ///
    /// flutter_rust_bridge:ignore
    pub fn as_datetime(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            Value::DateTime(s) => chrono::DateTime::parse_from_rfc3339(s)
                .ok() // ALLOW(ok): boundary parse
                .map(|dt| dt.with_timezone(&chrono::Utc)),
            _ => None,
        }
    }

    /// Create a Value from a chrono::DateTime
    pub fn from_datetime(dt: chrono::DateTime<chrono::Utc>) -> Self {
        Value::DateTime(dt.to_rfc3339())
    }

    /// Get array value
    ///
    /// flutter_rust_bridge:ignore
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(arr) => Some(arr),
            _ => None,
        }
    }

    /// Get object value
    ///
    /// flutter_rust_bridge:ignore
    pub fn as_object(&self) -> Option<&HashMap<String, Value>> {
        match self {
            Value::Object(obj) => Some(obj),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Create an empty Object (for serde default)
    pub fn default_object() -> Self {
        Value::Object(HashMap::new())
    }

    /// Create an empty Array (for serde default)
    pub fn default_array() -> Self {
        Value::Array(Vec::new())
    }

    pub fn to_display_string(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Integer(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Boolean(b) => b.to_string(),
            Value::DateTime(dt) => dt.clone(),
            Value::Null => String::new(),
            Value::Json(j) => j.clone(),
            Value::Array(items) => {
                let parts: Vec<String> = items.iter().map(|v| v.to_display_string()).collect();
                parts.join(", ")
            }
            Value::Object(map) => serde_json::to_string(map).unwrap_or_default(),
        }
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Boolean(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Integer(i)
    }
}

impl From<i32> for Value {
    fn from(i: i32) -> Self {
        Value::Integer(i as i64)
    }
}

impl From<u32> for Value {
    fn from(u: u32) -> Self {
        Value::Integer(u as i64)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Value::Float(f)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl<T> From<Vec<T>> for Value
where
    T: Into<Value>,
{
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(|x| x.into()).collect())
    }
}

impl<T> From<Option<T>> for Value
where
    T: Into<Value>,
{
    fn from(opt: Option<T>) -> Self {
        match opt {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

impl From<HashMap<String, Value>> for Value {
    fn from(map: HashMap<String, Value>) -> Self {
        Value::Object(map)
    }
}

impl TryFrom<Value> for bool {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Boolean(b) => Ok(b),
            Value::Integer(i) => Ok(i != 0),
            _ => Err("Value is not a boolean or integer".into()),
        }
    }
}

impl TryFrom<Value> for i64 {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        value
            .as_i64()
            .ok_or_else(|| "Value is not an integer".into())
    }
}

impl TryFrom<Value> for i32 {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        value
            .as_i64()
            .and_then(|i| i.try_into().ok()) // ALLOW(ok): i64→i32 range check
            .ok_or_else(|| "Value is not a valid i32".into())
    }
}

impl TryFrom<Value> for u32 {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        value
            .as_i64()
            .and_then(|i| i.try_into().ok()) // ALLOW(ok): i64→u32 range check
            .ok_or_else(|| "Value is not a valid u32".into())
    }
}

impl TryFrom<Value> for f64 {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        value.as_f64().ok_or_else(|| "Value is not a float".into())
    }
}

impl TryFrom<Value> for String {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => Ok(s),
            _ => Err("Value is not a string".into()),
        }
    }
}

impl<T> TryFrom<Value> for Option<T>
where
    T: TryFrom<Value, Error = Box<dyn std::error::Error + Send + Sync>>,
{
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        if value.is_null() {
            Ok(None)
        } else {
            T::try_from(value).map(Some)
        }
    }
}

impl<T> TryFrom<Value> for Vec<T>
where
    T: TryFrom<Value, Error = Box<dyn std::error::Error + Send + Sync>>,
{
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Array(arr) => arr.into_iter().map(T::try_from).collect(),
            Value::Json(s) | Value::String(s) => {
                if s.is_empty() {
                    return Ok(Vec::new());
                }
                let json: serde_json::Value = serde_json::from_str(&s).map_err(|e| {
                    format!("Vec<T>::try_from JSON parse failed for {:?}: {}", s, e)
                })?;
                match json {
                    serde_json::Value::Array(arr) => arr
                        .into_iter()
                        .map(|j| T::try_from(Value::from_json_value(j)))
                        .collect(),
                    other => {
                        Err(format!("Vec<T>::try_from expected JSON array, got {:?}", other).into())
                    }
                }
            }
            Value::Null => Ok(Vec::new()),
            other => Err(format!("Vec<T>::try_from cannot convert {:?}", other).into()),
        }
    }
}

impl From<serde_json::Value> for Value {
    fn from(v: serde_json::Value) -> Self {
        Value::from_json_value(v)
    }
}

impl From<Value> for serde_json::Value {
    fn from(v: Value) -> Self {
        match v {
            Value::String(s) => serde_json::Value::String(s),
            Value::Integer(i) => serde_json::Value::Number(serde_json::Number::from(i)),
            Value::Float(f) => serde_json::Number::from_f64(f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Boolean(b) => serde_json::Value::Bool(b),
            Value::DateTime(s) => serde_json::Value::String(s.clone()),
            Value::Json(s) => serde_json::from_str(&s).unwrap_or(serde_json::Value::Null),
            Value::Array(arr) => {
                serde_json::Value::Array(arr.into_iter().map(Into::into).collect())
            }
            Value::Object(obj) => {
                serde_json::Value::Object(obj.into_iter().map(|(k, v)| (k, v.into())).collect())
            }
            Value::Null => serde_json::Value::Null,
        }
    }
}
impl From<HashMap<String, String>> for Value {
    fn from(map: HashMap<String, String>) -> Self {
        Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
        )
    }
}

impl TryFrom<Value> for HashMap<String, String> {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Object(obj) => obj
                .into_iter()
                .map(|(k, v)| match v {
                    Value::String(s) => Ok((k, s)),
                    _ => Err(format!("Value for key '{}' is not a string", k).into()),
                })
                .collect(),
            Value::Null => Ok(HashMap::new()),
            _ => Err("Value is not an object".into()),
        }
    }
}

// ============================================================================
// Required trait implementations for HashMap<String, Value> to work with Entity
// macro
// ============================================================================

impl TryFrom<Value> for HashMap<String, Value> {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Object(obj) => Ok(obj),
            Value::Null => Ok(HashMap::new()),
            Value::Json(s) | Value::String(s) => {
                if s.is_empty() {
                    return Ok(HashMap::new());
                }
                let json: serde_json::Value = serde_json::from_str(&s).map_err(|e| {
                    format!(
                        "HashMap<String,Value>::try_from JSON parse failed for {:?}: {}",
                        s, e
                    )
                })?;
                match json {
                    serde_json::Value::Object(obj) => Ok(obj
                        .into_iter()
                        .map(|(k, v)| (k, Value::from_json_value(v)))
                        .collect()),
                    other => Err(format!(
                        "HashMap<String,Value>::try_from expected JSON object, got {:?}",
                        other
                    )
                    .into()),
                }
            }
            other => {
                Err(format!("HashMap<String,Value>::try_from cannot convert {:?}", other).into())
            }
        }
    }
}
