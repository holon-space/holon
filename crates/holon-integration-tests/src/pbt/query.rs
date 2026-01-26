//! Language-neutral query representation for PBT testing.
//!
//! `TestQuery` compiles to PRQL, SQL, or GQL and evaluates against the reference model.
//! Uses `holon_api::Predicate` directly — no separate TestPredicate type.

use std::collections::HashMap;

use holon_api::block::Block;
use holon_api::predicate::Predicate;
use holon_api::{EntityUri, QueryLanguage, Value};

/// Backward-compat alias — old code that references `TestPredicate` still compiles.
pub type TestPredicate = Predicate;

/// A watched query specification: query + language.
#[derive(Debug, Clone)]
pub struct WatchSpec {
    pub query: TestQuery,
    pub language: QueryLanguage,
}

/// Which table a TestQuery targets.
#[derive(Debug, Clone)]
pub enum QueryTable {
    Blocks,
}

/// A language-neutral query that can compile to PRQL, SQL, or GQL and also
/// evaluate against the reference model.
#[derive(Debug, Clone)]
pub struct TestQuery {
    pub table: QueryTable,
    pub columns: Vec<String>,
    pub predicates: Vec<Predicate>,
}

/// Format a Value for embedding in a SQL string.
pub fn value_to_sql_literal(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{s}'"),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Null => "NULL".to_string(),
        other => format!("'{:?}'", other),
    }
}

/// Format a Value for PRQL (uses double-quotes for strings).
pub fn value_to_prql_literal(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Null => "null".to_string(),
        other => format!("\"{:?}\"", other),
    }
}

fn pred_to_prql(pred: &Predicate) -> String {
    match pred {
        Predicate::Eq { field, value } => {
            format!("| filter {field} == {} ", value_to_prql_literal(value))
        }
        Predicate::Ne { field, value } if value.is_null() => {
            format!("| filter {field} != null ")
        }
        Predicate::Ne { field, value } => {
            format!("| filter {field} != {} ", value_to_prql_literal(value))
        }
        Predicate::IsNotNull(field) => format!("| filter {field} != null "),
        _ => String::new(),
    }
}

fn pred_to_sql_where(pred: &Predicate) -> String {
    match pred {
        Predicate::Eq { field, value } => {
            format!("{field} = {}", value_to_sql_literal(value))
        }
        Predicate::Ne { field, value } if value.is_null() => {
            format!("{field} IS NOT NULL")
        }
        Predicate::Ne { field, value } => {
            format!("{field} != {}", value_to_sql_literal(value))
        }
        Predicate::IsNotNull(field) => format!("{field} IS NOT NULL"),
        _ => "1=1".to_string(),
    }
}

fn pred_to_gql_where(pred: &Predicate) -> String {
    match pred {
        Predicate::Eq { field, value } => {
            format!("n.{field} = {}", value_to_sql_literal(value))
        }
        Predicate::Ne { field, value } if value.is_null() => {
            format!("n.{field} IS NOT NULL")
        }
        Predicate::Ne { field, value } => {
            format!("n.{field} <> {}", value_to_sql_literal(value))
        }
        Predicate::IsNotNull(field) => format!("n.{field} IS NOT NULL"),
        _ => "1=1".to_string(),
    }
}

impl TestQuery {
    pub fn to_prql(&self) -> String {
        let cols = self.columns.join(", ");
        let mut q = format!("from block | select {{{cols}}} ");
        for pred in &self.predicates {
            q.push_str(&pred_to_prql(pred));
        }
        q
    }

    pub fn to_sql(&self) -> String {
        let cols = self.columns.join(", ");
        let mut q = format!("SELECT {cols} FROM block");
        let wheres: Vec<String> = self.predicates.iter().map(pred_to_sql_where).collect();
        if !wheres.is_empty() {
            q.push_str(" WHERE ");
            q.push_str(&wheres.join(" AND "));
        }
        q
    }

    pub fn to_gql(&self) -> String {
        let returns = self
            .columns
            .iter()
            .map(|c| format!("n.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let wheres: Vec<String> = self.predicates.iter().map(pred_to_gql_where).collect();
        let where_clause = if wheres.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", wheres.join(" AND "))
        };
        format!("MATCH (n:block){where_clause} RETURN {returns}")
    }

    /// Compile to the given language.
    pub fn compile_for(&self, lang: QueryLanguage) -> (String, QueryLanguage) {
        match lang {
            QueryLanguage::HolonPrql => (self.to_prql(), QueryLanguage::HolonPrql),
            QueryLanguage::HolonSql => (self.to_sql(), QueryLanguage::HolonSql),
            QueryLanguage::HolonGql => (self.to_gql(), QueryLanguage::HolonGql),
        }
    }

    /// Evaluate this query against the reference model's blocks.
    pub fn evaluate(&self, blocks: &HashMap<EntityUri, Block>) -> Vec<HashMap<String, Value>> {
        blocks
            .values()
            .filter(|b| self.predicates.iter().all(|p| predicate_matches(p, b)))
            .map(|b| self.project_columns(b))
            .collect()
    }

    fn project_columns(&self, block: &Block) -> HashMap<String, Value> {
        let mut row = HashMap::new();
        for col in &self.columns {
            let val = match col.as_str() {
                "id" => Value::String(block.id.to_string()),
                "content" => Value::String(block.content.clone()),
                "content_type" => block.content_type.into(),
                "parent_id" => block.parent_id.clone().into(),
                "source_language" => match &block.source_language {
                    Some(sl) => Value::String(sl.to_string()),
                    None => Value::Null,
                },
                "source_name" => match &block.source_name {
                    Some(sn) => Value::String(sn.clone()),
                    None => Value::Null,
                },
                _ => block.properties.get(col).cloned().unwrap_or(Value::Null),
            };
            row.insert(col.clone(), val);
        }
        row
    }
}

/// Evaluate a Predicate against a Block (for PBT reference model).
pub fn predicate_matches(pred: &Predicate, block: &Block) -> bool {
    let get_field = |field: &str| -> Option<Value> {
        match field {
            "id" => Some(Value::String(block.id.to_string())),
            "content" => Some(Value::String(block.content.clone())),
            "content_type" => Some(block.content_type.into()),
            "parent_id" => Some(block.parent_id.clone().into()),
            "source_language" => block
                .source_language
                .as_ref()
                .map(|sl| Value::String(sl.to_string())),
            "source_name" => block
                .source_name
                .as_ref()
                .map(|sn| Value::String(sn.clone())),
            other => block.properties.get(other).cloned(),
        }
    };

    let compare_numeric = |field: &str, value: &Value, cmp: fn(f64, f64) -> bool| -> bool {
        let Some(lhs) = get_field(field) else {
            return false;
        };
        let (Some(l), Some(r)) = (lhs.as_f64(), value.as_f64()) else {
            return false;
        };
        cmp(l, r)
    };

    match pred {
        Predicate::Eq { field, value } => get_field(field).as_ref() == Some(value),
        Predicate::Ne { field, value } => {
            match (get_field(field), value) {
                (None, Value::Null) => false, // NULL != NULL → false in SQL
                (None, _) => true,            // NULL != non-null → true
                (Some(v), _) => &v != value,
            }
        }
        Predicate::Gt { field, value } => compare_numeric(field, value, |l, r| l > r),
        Predicate::Lt { field, value } => compare_numeric(field, value, |l, r| l < r),
        Predicate::Gte { field, value } => compare_numeric(field, value, |l, r| l >= r),
        Predicate::Lte { field, value } => compare_numeric(field, value, |l, r| l <= r),
        Predicate::IsNotNull(field) => get_field(field).is_some(),
        Predicate::Var(field) => get_field(field).is_some_and(|v| !v.is_null()),
        Predicate::Not(inner) => !predicate_matches(inner, block),
        Predicate::And(preds) => preds.iter().all(|p| predicate_matches(p, block)),
        Predicate::Or(preds) => preds.iter().any(|p| predicate_matches(p, block)),
        Predicate::Always => true,
    }
}
