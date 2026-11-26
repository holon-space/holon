use crate::Discipline;
use crate::Kinds;
use crate::Out;

/// What a reader sees of one row.
#[derive(Debug)]
pub enum RowView<'a, K: Kinds> {
    /// Head first. Empty when the row holds nothing.
    Tokens(Vec<&'a K::Token>),
    Count(u32),
}

impl<K: Kinds> RowView<'_, K> {
    pub fn is_empty(&self) -> bool {
        match self {
            RowView::Tokens(t) => t.is_empty(),
            RowView::Count(n) => *n == 0,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            RowView::Tokens(t) => t.len(),
            RowView::Count(n) => *n as usize,
        }
    }
}

/// One firing as the store applies it: first every consume (the head token,
/// or one unit), then every output in order.
#[derive(Clone, Debug)]
pub struct Applied<K: Kinds> {
    pub consumes: Vec<(K::Place, K::Key)>,
    pub outputs: Vec<(K::Place, K::Key, Out<K>)>,
}

/// The durable marking.
///
/// Laws every store satisfies (checked by `testkit::check_store_laws`):
/// - stamp-sound: equal row stamps mean equal row content;
/// - move-neutral: equal row content reached only by moves gives equal row
///   stamps, so a token that moves out and back leaves the row stamp as it was.
///
/// `apply` is atomic. Its caller has checked the firing against the
/// disciplines; a store asserts them.
pub trait Store<K: Kinds> {
    /// No `Ord`: stamps of replicas have no total order.
    type Stamp: Clone + Eq + std::fmt::Debug;
    type RowStamp: Clone + Eq + std::fmt::Debug;

    fn row(&self, place: &K::Place, key: &K::Key) -> RowView<'_, K>;
    fn row_stamp(&self, place: &K::Place, key: &K::Key) -> Self::RowStamp;
    /// Keys of the non-empty rows of `place`, ascending.
    fn keys(&self, place: &K::Place) -> Vec<K::Key>;
    /// Ascending by place.
    fn places(&self) -> Vec<(K::Place, Discipline)>;
    /// Returns the stamps of the `New` outputs, in output order.
    fn apply(&mut self, firing: Applied<K>) -> Vec<Self::Stamp>;
}
