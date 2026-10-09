//! The file each document's page lives in, indexed by [`PathCollisionKey`]
//! so "which other document homes a spelling of this path" is a lookup, not
//! a fold of every home.

use std::collections::BTreeSet;
use std::collections::HashMap;

use holon_api::EntityUri;
use holon_core::CanonicalPath;

use crate::vault_path::PathCollisionKey;

#[derive(Default)]
pub(crate) struct DocHomes {
    by_doc: HashMap<EntityUri, CanonicalPath>,
    by_key: HashMap<PathCollisionKey, BTreeSet<EntityUri>>,
}

impl DocHomes {
    pub(crate) fn get(&self, doc: &EntityUri) -> Option<&CanonicalPath> {
        self.by_doc.get(doc)
    }

    pub(crate) fn contains_key(&self, doc: &EntityUri) -> bool {
        self.by_doc.contains_key(doc)
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &CanonicalPath> {
        self.by_doc.values()
    }

    pub(crate) fn insert(&mut self, doc: EntityUri, home: CanonicalPath) {
        let key = PathCollisionKey::of(home.as_path_buf());
        if let Some(old) = self.by_doc.insert(doc.clone(), home) {
            self.unindex(&doc, &old);
        }
        self.by_key.entry(key).or_default().insert(doc);
    }

    /// Forget every document homed at `home`; returns them.
    pub(crate) fn remove_at(&mut self, home: &CanonicalPath) -> Vec<EntityUri> {
        let gone = self.docs_at(home);
        for doc in &gone {
            self.by_doc.remove(doc);
            self.unindex(doc, home);
        }
        gone
    }

    /// Re-home every document homed at `from` to `to`; returns them.
    pub(crate) fn move_homes(
        &mut self,
        from: &CanonicalPath,
        to: &CanonicalPath,
    ) -> Vec<EntityUri> {
        let moved = self.docs_at(from);
        for doc in &moved {
            self.insert(doc.clone(), to.clone());
        }
        moved
    }

    /// A document other than `doc` whose home shares `key`, with that home.
    pub(crate) fn other_at_key(
        &self,
        doc: &EntityUri,
        key: &PathCollisionKey,
    ) -> Option<(&EntityUri, &CanonicalPath)> {
        let other = self.by_key.get(key)?.iter().find(|other| *other != doc)?;
        Some((other, &self.by_doc[other]))
    }

    fn docs_at(&self, home: &CanonicalPath) -> Vec<EntityUri> {
        self.by_key
            .get(&PathCollisionKey::of(home.as_path_buf()))
            .into_iter()
            .flatten()
            .filter(|doc| self.by_doc.get(*doc) == Some(home))
            .cloned()
            .collect()
    }

    fn unindex(&mut self, doc: &EntityUri, home: &CanonicalPath) {
        let key = PathCollisionKey::of(home.as_path_buf());
        let docs = self
            .by_key
            .get_mut(&key)
            .expect("every recorded home is indexed under its key");
        assert!(
            docs.remove(doc),
            "{doc} was not indexed under its home's key"
        );
        if docs.is_empty() {
            self.by_key.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn home(p: &str) -> CanonicalPath {
        CanonicalPath::new(&PathBuf::from(p))
    }

    #[test]
    fn a_spelling_of_another_documents_home_is_found_and_follows_its_moves() {
        let (a, b) = (EntityUri::block("a"), EntityUri::block("b"));
        let mut homes = DocHomes::default();
        homes.insert(a.clone(), home("/holon-virtual/v/My Notes.org"));
        let key = PathCollisionKey::of(&PathBuf::from("/holon-virtual/v/MY NOTES.org"));
        assert_eq!(homes.other_at_key(&b, &key).map(|(d, _)| d), Some(&a));
        assert_eq!(homes.other_at_key(&a, &key), None);

        let moved = homes.move_homes(
            &home("/holon-virtual/v/My Notes.org"),
            &home("/holon-virtual/v/Other.org"),
        );
        assert_eq!(moved, vec![a.clone()]);
        assert_eq!(homes.other_at_key(&b, &key), None);

        homes.insert(b.clone(), home("/holon-virtual/v/Other.org"));
        assert_eq!(
            homes.remove_at(&home("/holon-virtual/v/Other.org")),
            vec![a.clone(), b.clone()]
        );
        assert!(!homes.contains_key(&a) && homes.by_key.is_empty());
    }
}
