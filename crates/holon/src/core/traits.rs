use async_trait::async_trait;
use std::fmt::Debug;
use std::sync::Arc;

use holon_api::Value;
use holon_api::predicate::Predicate as PredicateEnum;
use turso;

// Re-export schema types from holon_api to avoid duplication
pub use holon_api::{DynamicEntity, FieldSchema, IntoEntity, TryFromEntity, TypeDefinition};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Generate SQL from a `Predicate` enum.
///
/// Separate from the `Predicate` type (which lives in holon-api) because
/// SQL generation depends on the turso crate (in holon, not holon-api).
pub trait ToSql {
    fn to_sql_predicate(&self) -> Option<SqlPredicate>;
}

impl ToSql for PredicateEnum {
    fn to_sql_predicate(&self) -> Option<SqlPredicate> {
        match self {
            PredicateEnum::Eq { field, value } => Some(SqlPredicate::new(
                format!("{field} = ?"),
                vec![value.clone()],
            )),
            PredicateEnum::Ne { field, value } => {
                if value.is_null() {
                    Some(SqlPredicate::new(format!("{field} IS NOT NULL"), vec![]))
                } else {
                    Some(SqlPredicate::new(
                        format!("{field} != ?"),
                        vec![value.clone()],
                    ))
                }
            }
            PredicateEnum::Gt { field, value } => Some(SqlPredicate::new(
                format!("{field} > ?"),
                vec![value.clone()],
            )),
            PredicateEnum::Lt { field, value } => Some(SqlPredicate::new(
                format!("{field} < ?"),
                vec![value.clone()],
            )),
            PredicateEnum::Gte { field, value } => Some(SqlPredicate::new(
                format!("{field} >= ?"),
                vec![value.clone()],
            )),
            PredicateEnum::Lte { field, value } => Some(SqlPredicate::new(
                format!("{field} <= ?"),
                vec![value.clone()],
            )),
            PredicateEnum::IsNotNull(field) => {
                Some(SqlPredicate::new(format!("{field} IS NOT NULL"), vec![]))
            }
            PredicateEnum::Var(field) => Some(SqlPredicate::new(
                format!("{field} IS NOT NULL AND {field} != '' AND {field} != 0"),
                vec![],
            )),
            PredicateEnum::Not(inner) => inner
                .to_sql_predicate()
                .map(|p| SqlPredicate::new(format!("NOT ({})", p.sql), p.params)),
            PredicateEnum::And(preds) => {
                let parts: Vec<SqlPredicate> =
                    preds.iter().filter_map(|p| p.to_sql_predicate()).collect();
                if parts.len() != preds.len() {
                    return None;
                }
                let sql = parts
                    .iter()
                    .map(|p| format!("({})", p.sql))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let params = parts.into_iter().flat_map(|p| p.params).collect();
                Some(SqlPredicate::new(sql, params))
            }
            PredicateEnum::Or(preds) => {
                let parts: Vec<SqlPredicate> =
                    preds.iter().filter_map(|p| p.to_sql_predicate()).collect();
                if parts.len() != preds.len() {
                    return None;
                }
                let sql = parts
                    .iter()
                    .map(|p| format!("({})", p.sql))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                let params = parts.into_iter().flat_map(|p| p.params).collect();
                Some(SqlPredicate::new(sql, params))
            }
            PredicateEnum::Always => None,
        }
    }
}

/// Convert a holon_api::Value to turso::Value for database operations.
/// This handles all Value variants including Object and Array by serializing them to JSON.
pub fn value_to_turso(value: &Value) -> turso::Value {
    match value {
        Value::String(s) => turso::Value::Text(s.clone()),
        Value::Integer(i) => turso::Value::Integer(*i),
        Value::Float(f) => turso::Value::Real(*f),
        Value::Boolean(b) => turso::Value::Integer(if *b { 1 } else { 0 }),
        Value::Null => turso::Value::Null,
        // DateTime, Json, Reference, Object, Array all serialize to JSON text
        v => turso::Value::Text(v.to_json_string()),
    }
}

pub trait Lens<T, U>: Clone + Send + Sync + 'static {
    fn get(&self, source: &T) -> Option<U>;
    fn set(&self, source: &mut T, value: U);
    fn sql_column(&self) -> &'static str {
        self.field_name()
    }
    fn field_name(&self) -> &'static str;
}

pub trait Predicate<T>: Send + Sync {
    fn test(&self, item: &T) -> bool;
    fn to_sql(&self) -> Option<SqlPredicate>;

    fn and<P>(self, other: P) -> And<T, Self, P>
    where
        Self: Sized,
        P: Predicate<T>,
    {
        And {
            left: self,
            right: other,
            _phantom: std::marker::PhantomData,
        }
    }

    fn or<P>(self, other: P) -> Or<T, Self, P>
    where
        Self: Sized,
        P: Predicate<T>,
    {
        Or {
            left: self,
            right: other,
            _phantom: std::marker::PhantomData,
        }
    }

    fn not(self) -> Not<T, Self>
    where
        Self: Sized,
    {
        Not {
            inner: self,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> Predicate<T> for Arc<dyn Predicate<T>>
where
    T: Send + Sync,
{
    fn test(&self, item: &T) -> bool {
        (**self).test(item)
    }

    fn to_sql(&self) -> Option<SqlPredicate> {
        (**self).to_sql()
    }
}

#[derive(Debug, Clone)]
pub struct SqlPredicate {
    pub sql: String,
    pub params: Vec<Value>,
}

impl SqlPredicate {
    pub fn new(sql: String, params: Vec<Value>) -> Self {
        Self { sql, params }
    }

    pub fn to_params(&self) -> Vec<turso::Value> {
        self.params.iter().map(value_to_turso).collect()
    }
}

pub struct And<T, L, R> {
    left: L,
    right: R,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, L, R> Predicate<T> for And<T, L, R>
where
    T: Send + Sync,
    L: Predicate<T>,
    R: Predicate<T>,
{
    fn test(&self, item: &T) -> bool {
        self.left.test(item) && self.right.test(item)
    }

    fn to_sql(&self) -> Option<SqlPredicate> {
        match (self.left.to_sql(), self.right.to_sql()) {
            (Some(left), Some(right)) => {
                let mut params = left.params;
                params.extend(right.params);
                Some(SqlPredicate::new(
                    format!("({}) AND ({})", left.sql, right.sql),
                    params,
                ))
            }
            _ => None,
        }
    }
}

pub struct Or<T, L, R> {
    left: L,
    right: R,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, L, R> Predicate<T> for Or<T, L, R>
where
    T: Send + Sync,
    L: Predicate<T>,
    R: Predicate<T>,
{
    fn test(&self, item: &T) -> bool {
        self.left.test(item) || self.right.test(item)
    }

    fn to_sql(&self) -> Option<SqlPredicate> {
        match (self.left.to_sql(), self.right.to_sql()) {
            (Some(left), Some(right)) => {
                let mut params = left.params;
                params.extend(right.params);
                Some(SqlPredicate::new(
                    format!("({}) OR ({})", left.sql, right.sql),
                    params,
                ))
            }
            _ => None,
        }
    }
}

pub struct Not<T, P> {
    inner: P,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, P> Predicate<T> for Not<T, P>
where
    T: Send + Sync,
    P: Predicate<T>,
{
    fn test(&self, item: &T) -> bool {
        !self.inner.test(item)
    }

    fn to_sql(&self) -> Option<SqlPredicate> {
        self.inner
            .to_sql()
            .map(|pred| SqlPredicate::new(format!("NOT ({})", pred.sql), pred.params))
    }
}

#[async_trait]
pub trait Queryable<T>: Send + Sync
where
    T: Send + Sync + 'static,
{
    async fn query<P>(&self, predicate: P) -> Result<Vec<T>>
    where
        P: Predicate<T> + Send + 'static;
}

/// Result of an incremental sync operation
#[derive(Debug, Clone)]
pub struct SyncResult<T, Token> {
    /// All items from sync (for full sync) or changed items (for incremental)
    pub items: Vec<T>,
    /// Items that were updated (empty for full sync, populated for incremental)
    pub updated: Vec<T>,
    /// IDs of deleted items (empty for full sync, populated for incremental)
    pub deleted: Vec<String>,
    /// Token for next incremental sync (None if no more updates available)
    pub next_token: Option<Token>,
}

// TypeDefinition, FieldSchema, IntoEntity, TryFromEntity are re-exported from holon_api above

#[cfg(test)]
mod tests {
    use super::*;

    struct TestItem {
        value: i64,
    }

    struct TestPredicate;

    impl Predicate<TestItem> for TestPredicate {
        fn test(&self, item: &TestItem) -> bool {
            item.value > 10
        }

        fn to_sql(&self) -> Option<SqlPredicate> {
            Some(SqlPredicate::new(
                "value > ?".to_string(),
                vec![Value::Integer(10)],
            ))
        }
    }

    #[test]
    fn test_predicate_and() {
        let item = TestItem { value: 15 };

        let pred = TestPredicate.and(TestPredicate);
        assert!(pred.test(&item));
    }

    #[test]
    fn test_predicate_or() {
        let item = TestItem { value: 5 };

        let pred = TestPredicate.or(TestPredicate);
        assert!(!pred.test(&item));
    }

    #[test]
    fn test_predicate_not() {
        let item = TestItem { value: 5 };

        let pred = TestPredicate.not();
        assert!(pred.test(&item));
    }

    #[test]
    fn test_sql_generation() {
        let pred = TestPredicate.and(TestPredicate);
        let sql = pred.to_sql().unwrap();
        assert_eq!(sql.sql, "(value > ?) AND (value > ?)");
        assert_eq!(sql.params.len(), 2);
    }

    #[test]
    fn test_type_definition_to_sql() {
        let td = TypeDefinition::new(
            "tasks",
            vec![
                FieldSchema::new("id", "TEXT").primary_key(),
                FieldSchema::new("title", "TEXT"),
                FieldSchema::new("priority", "INTEGER").indexed().nullable(),
            ],
        );

        let sql = td.to_create_table_sql();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"tasks\""));
        assert!(sql.contains("id TEXT PRIMARY KEY"));
        assert!(sql.contains("title TEXT NOT NULL"));
        assert!(sql.contains("priority INTEGER"));

        let indexes = td.to_index_sql();
        assert_eq!(indexes.len(), 1);
        assert!(indexes[0].contains("CREATE INDEX IF NOT EXISTS idx_tasks_priority"));
    }
}
