//! Language-neutral query representation for PBT testing.
//!
//! `TestQuery` compiles to PRQL, SQL, or GQL and evaluates against the reference model.

use std::collections::HashMap;

use holon_api::block::Block;
use holon_api::{QueryLanguage, Value};

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

/// A single filter predicate for a TestQuery.
#[derive(Debug, Clone)]
pub enum TestPredicate {
    Eq(String, Value),
    Neq(String, Value),
    IsNotNull(String),
}

/// A language-neutral query that can compile to PRQL, SQL, or GQL and also
/// evaluate against the reference model.
#[derive(Debug, Clone)]
pub struct TestQuery {
    pub table: QueryTable,
    pub columns: Vec<String>,
    pub predicates: Vec<TestPredicate>,
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

impl TestQuery {
    pub fn to_prql(&self) -> String {
        let cols = self.columns.join(", ");
        let mut q = format!("from block | select {{{cols}}} ");
        for pred in &self.predicates {
            match pred {
                TestPredicate::Eq(field, val) => {
                    q.push_str(&format!(
                        "| filter {field} == {} ",
                        value_to_prql_literal(val)
                    ));
                }
                TestPredicate::Neq(field, Value::Null) => {
                    q.push_str(&format!("| filter {field} != null "));
                }
                TestPredicate::Neq(field, val) => {
                    q.push_str(&format!(
                        "| filter {field} != {} ",
                        value_to_prql_literal(val)
                    ));
                }
                TestPredicate::IsNotNull(field) => {
                    q.push_str(&format!("| filter {field} != null "));
                }
            }
        }
        q
    }

    pub fn to_sql(&self) -> String {
        let cols = self.columns.join(", ");
        let mut q = format!("SELECT {cols} FROM block");
        let mut wheres = Vec::new();
        for pred in &self.predicates {
            match pred {
                TestPredicate::Eq(field, val) => {
                    wheres.push(format!("{field} = {}", value_to_sql_literal(val)));
                }
                TestPredicate::Neq(field, Value::Null) => {
                    wheres.push(format!("{field} IS NOT NULL"));
                }
                TestPredicate::Neq(field, val) => {
                    wheres.push(format!("{field} != {}", value_to_sql_literal(val)));
                }
                TestPredicate::IsNotNull(field) => {
                    wheres.push(format!("{field} IS NOT NULL"));
                }
            }
        }
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
        let mut wheres = Vec::new();
        for pred in &self.predicates {
            match pred {
                TestPredicate::Eq(field, val) => {
                    wheres.push(format!("n.{field} = {}", value_to_sql_literal(val)));
                }
                TestPredicate::Neq(field, Value::Null) => {
                    wheres.push(format!("n.{field} IS NOT NULL"));
                }
                TestPredicate::Neq(field, val) => {
                    wheres.push(format!("n.{field} <> {}", value_to_sql_literal(val)));
                }
                TestPredicate::IsNotNull(field) => {
                    wheres.push(format!("n.{field} IS NOT NULL"));
                }
            }
        }
        let where_clause = if wheres.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", wheres.join(" AND "))
        };
        format!("MATCH (n:Block){where_clause} RETURN {returns}")
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
    pub fn evaluate(&self, blocks: &HashMap<String, Block>) -> Vec<HashMap<String, Value>> {
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
                "parent_id" => Value::String(block.parent_id.to_string()),
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

pub fn predicate_matches(pred: &TestPredicate, block: &Block) -> bool {
    let get_field = |field: &str| -> Option<Value> {
        match field {
            "id" => Some(Value::String(block.id.to_string())),
            "content" => Some(Value::String(block.content.clone())),
            "content_type" => Some(block.content_type.into()),
            "parent_id" => Some(Value::String(block.parent_id.to_string())),
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

    match pred {
        TestPredicate::Eq(field, expected) => get_field(field).as_ref() == Some(expected),
        TestPredicate::Neq(field, expected) => {
            match (get_field(field), expected) {
                (None, Value::Null) => false, // NULL != NULL → false in SQL
                (None, _) => true,            // NULL != non-null → true
                (Some(v), _) => &v != expected,
            }
        }
        TestPredicate::IsNotNull(field) => get_field(field).is_some(),
    }
}
