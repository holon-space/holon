//! A [`LiveData`] mirror, exposed as a named [`RowSource`].
//!
//! This is the second half of the `live_query` seam: [`row_source`] parses a
//! builder's arguments into a spec, and this module serves the `Named` arm from
//! an in-process holder rather than a query. Settings › Integrations and the
//! conditions surfaces are collections over state no query produces, so without
//! it they would each need their own bespoke widget.
//!
//! Typed stays typed: `T` is only flattened into a [`DataRow`] by the `to_row`
//! map this source is registered with, and that map is the single place a
//! column name is minted. A filter names one of the [`NamedSourceDef`] columns
//! or the build fails, so a filter and the map can never disagree about what a
//! row contains.
//!
//! [`row_source`]: crate::row_source

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal_vec::SignalVec;
use futures_signals::signal_vec::SignalVecExt as _;

use crate::Occurrence;
use crate::RowKey;
use crate::Value;
use crate::interp_value::ReactiveRowProvider;
use crate::live_data::LiveData;
use crate::row_source::NamedSourceDef;
use crate::row_source::RowFilter;
use crate::row_source::RowSource;
use crate::widget_spec::DataRow;

/// Turns one mirrored item into the row a widget reads.
///
/// It takes the mirror key as well as the value because the key is the display
/// order ([`LiveData::entries_signal_vec`] emits in key order) and, for a
/// composite key, the only place the grouping field survives.
pub type RowMap<T> = Arc<dyn Fn(&str, &T) -> DataRow + Send + Sync>;

/// The column every row must carry: collections key their per-row identity off
/// it, so a map that omits it produces rows no widget can address.
pub const ID_COLUMN: &str = "id";

/// A registered named source backed by a [`LiveData`] mirror.
pub struct LiveDataSource<T: Clone + Send + Sync + 'static> {
    def: NamedSourceDef,
    data: Arc<LiveData<T>>,
    to_row: RowMap<T>,
}

impl<T: Clone + Send + Sync + 'static> LiveDataSource<T> {
    /// Declare `data` as the source called `def.name`.
    ///
    /// `def.columns` must include [`ID_COLUMN`] — asserted here rather than
    /// discovered as a widget that renders rows it cannot key.
    pub fn new(def: NamedSourceDef, data: Arc<LiveData<T>>, to_row: RowMap<T>) -> Arc<Self> {
        assert!(
            def.columns.contains(&ID_COLUMN),
            "named source '{}' declares no '{ID_COLUMN}' column, so its rows cannot be keyed",
            def.name
        );
        Arc::new(Self { def, data, to_row })
    }
}

impl<T: Clone + Send + Sync + 'static> RowSource for LiveDataSource<T> {
    fn def(&self) -> NamedSourceDef {
        self.def
    }

    fn provider(&self, filter: Option<&RowFilter>) -> Arc<dyn ReactiveRowProvider> {
        Arc::new(LiveDataProvider {
            name: self.def.name,
            data: Arc::clone(&self.data),
            to_row: Arc::clone(&self.to_row),
            filter: filter.cloned(),
        })
    }
}

/// The rows of one [`LiveDataSource`] under one filter.
///
/// A pure function of the mirror and the filter, as
/// [`ReactiveRowProvider`] requires: it accumulates nothing, so a fresh
/// construction is observationally the previous one and the provider cache's
/// `Weak` lifecycle stays safe.
struct LiveDataProvider<T: Clone + Send + Sync + 'static> {
    name: &'static str,
    data: Arc<LiveData<T>>,
    to_row: RowMap<T>,
    filter: Option<RowFilter>,
}

impl<T: Clone + Send + Sync + 'static> LiveDataProvider<T> {
    fn passes(filter: &Option<RowFilter>, row: &DataRow) -> bool {
        let Some(filter) = filter else {
            return true;
        };
        row.get(filter.column().as_str())
            .and_then(Value::as_string)
            .is_some_and(|v| v == filter.equals())
    }

    fn row_of(&self, key: &str, value: &T) -> Arc<DataRow> {
        Arc::new((self.to_row)(key, value))
    }
}

/// The key one mirrored row is addressed by.
///
/// Unlike a query's `id`, this column is minted by the source's own
/// [`RowMap`] from the mirror key, so a value that names no entity is a
/// registration bug — and keying every such row on one default would collapse
/// the whole mirror onto a single entry.
fn row_key(row: &DataRow) -> RowKey {
    match crate::widget_spec::row_id_of(row) {
        crate::widget_spec::RowId::Entity(uri) => (uri, Occurrence::Canonical),
        other => panic!(
            "a named source's row map minted a '{ID_COLUMN}' column that names no entity: \
             {other:?}"
        ),
    }
}

impl<T: Clone + Send + Sync + 'static> ReactiveRowProvider for LiveDataProvider<T> {
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
        let items: BTreeMap<String, Arc<T>> = self.data.read().clone();
        items
            .iter()
            .map(|(k, v)| self.row_of(k, v))
            .filter(|row| Self::passes(&self.filter, row))
            .collect()
    }

    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
        let to_row = Arc::clone(&self.to_row);
        let filter = self.filter.clone();
        Box::pin(
            self.data
                .entries_signal_vec()
                .map(move |(k, v)| Arc::new(to_row(&k, &v)))
                .filter(move |row| Self::passes(&filter, row)),
        )
    }

    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (RowKey, Arc<DataRow>)> + Send>> {
        let to_row = Arc::clone(&self.to_row);
        let filter = self.filter.clone();
        Box::pin(
            self.data
                .entries_signal_vec()
                .map(move |(k, v)| Arc::new(to_row(&k, &v)))
                .filter(move |row| Self::passes(&filter, row))
                .map(|row| (row_key(&row), row)),
        )
    }

    /// The source name plus the filter, hashed — NOT the allocation address.
    /// Two providers built from one spec are the same rows, and the cache's
    /// de-dup contract asks equal providers to say so.
    fn cache_identity(&self) -> u64 {
        use std::hash::Hash as _;
        use std::hash::Hasher as _;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.name.hash(&mut h);
        if let Some(filter) = &self.filter {
            filter.column().as_str().hash(&mut h);
            filter.equals().hash(&mut h);
        }
        h.finish()
    }
}
