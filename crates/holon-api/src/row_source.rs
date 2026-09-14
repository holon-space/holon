//! Where a collection's rows come from, as a parsed value.
//!
//! The `live_query` builder used to read `sql` / `gql` / `prql` inline and hand
//! the text straight to the query capability, which made "a collection" and "a
//! query result" the same thing. They are not: Settings › Integrations and the
//! sidebar are collections over state that no query produces. This module is
//! the seam — the builder parses its arguments into a [`RowSourceSpec`] once,
//! and everything downstream reads the spec instead of re-reading arguments.
//!
//! Parsing happens at the builder boundary and fails LOUDLY: an unknown source
//! name, or a filter over a column the source does not declare, is a build-time
//! error, never an empty row list. An empty list is indistinguishable from "the
//! query matched nothing", which is exactly the silent failure this replaces.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::QueryContext;
use crate::QueryLanguage;
use crate::interp_value::ReactiveRowProvider;

/// The name of a registered named source.
///
/// Constructible only by [`RowSourceRegistry::parse_named`], so a
/// `SourceName` in hand is proof the source is registered. Holding the
/// `&'static str` from the registry's own definition — not a caller's string —
/// means the name cannot drift from what was looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceName(&'static str);

impl SourceName {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for SourceName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// A filter over one declared column of a named source.
///
/// The column is a [`ColumnName`], so it too is proof rather than a promise:
/// the only way to get one is to have it checked against the source's declared
/// columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowFilter {
    column: ColumnName,
    equals: String,
}

impl RowFilter {
    pub const fn column(&self) -> ColumnName {
        self.column
    }

    pub fn equals(&self) -> &str {
        &self.equals
    }
}

/// A column a named source declares it produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColumnName(&'static str);

impl ColumnName {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for ColumnName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Where a collection's rows come from.
#[derive(Clone, Debug, PartialEq)]
pub enum RowSourceSpec {
    /// A query in one of the query languages. The SQL pump serves this arm.
    Query {
        lang: QueryLanguage,
        text: String,
        context: Option<QueryContext>,
    },
    /// A registered in-process holder, mapped to rows.
    Named {
        source: SourceName,
        filter: Option<RowFilter>,
    },
}

/// What a named source promises about itself.
///
/// `columns` is the contract the `T → DataRow` map must keep: a filter names
/// one of these or the build fails, and nothing else is filterable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedSourceDef {
    pub name: &'static str,
    pub columns: &'static [&'static str],
}

/// One named source: its declaration, and the provider it produces.
pub trait RowSource: Send + Sync {
    fn def(&self) -> NamedSourceDef;

    /// The provider for this source under `filter`.
    fn provider(&self, filter: Option<&RowFilter>) -> Arc<dyn ReactiveRowProvider>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum RowSourceError {
    UnknownSource {
        asked: String,
        known: Vec<&'static str>,
    },
    UnknownColumn {
        source: &'static str,
        asked: String,
        declared: Vec<&'static str>,
    },
    DuplicateRegistration {
        name: &'static str,
    },
}

impl std::fmt::Display for RowSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSource { asked, known } => write!(
                f,
                "[unknown row source '{asked}' — registered sources: {}]",
                known.join(", ")
            ),
            Self::UnknownColumn {
                source,
                asked,
                declared,
            } => write!(
                f,
                "[row source '{source}' does not produce column '{asked}' — it produces: {}]",
                declared.join(", ")
            ),
            Self::DuplicateRegistration { name } => {
                write!(f, "[row source '{name}' is already registered]")
            }
        }
    }
}

impl std::error::Error for RowSourceError {}

/// Every named source the frontend can build a collection over.
#[derive(Default)]
pub struct RowSourceRegistry {
    sources: BTreeMap<&'static str, Arc<dyn RowSource>>,
}

impl RowSourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one source. A duplicate name is an error rather than a silent
    /// overwrite: two holders answering to one name is a bug that would
    /// otherwise surface as rows from whichever registered last.
    pub fn register(&mut self, source: Arc<dyn RowSource>) -> Result<(), RowSourceError> {
        let name = source.def().name;
        if self.sources.contains_key(name) {
            return Err(RowSourceError::DuplicateRegistration { name });
        }
        self.sources.insert(name, source);
        Ok(())
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.sources.keys().copied().collect()
    }

    pub fn get(&self, name: SourceName) -> &Arc<dyn RowSource> {
        self.sources
            .get(name.as_str())
            .expect("a SourceName is only constructible from this registry")
    }

    /// Parse a builder's `source:` (and optional `where_column:` /
    /// `where_equals:`) arguments into a [`RowSourceSpec::Named`].
    pub fn parse_named(
        &self,
        source: &str,
        filter: Option<(&str, &str)>,
    ) -> Result<RowSourceSpec, RowSourceError> {
        let Some(registered) = self.sources.get(source) else {
            return Err(RowSourceError::UnknownSource {
                asked: source.to_string(),
                known: self.names(),
            });
        };
        let def = registered.def();
        let name = SourceName(def.name);

        let filter = match filter {
            None => None,
            Some((column, equals)) => {
                let Some(declared) = def.columns.iter().find(|c| **c == column) else {
                    return Err(RowSourceError::UnknownColumn {
                        source: def.name,
                        asked: column.to_string(),
                        declared: def.columns.to_vec(),
                    });
                };
                Some(RowFilter {
                    column: ColumnName(declared),
                    equals: equals.to_string(),
                })
            }
        };

        Ok(RowSourceSpec::Named {
            source: name,
            filter,
        })
    }
}
