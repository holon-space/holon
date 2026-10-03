//! Block URIs as dense [`Id`]s, the form rows carry them in.

use std::collections::HashMap;

use holon_api::EntityUri;

use crate::row::Id;

/// Grow-only: a deleted block keeps its id, so a row fed again after a delete
/// carries the same id.
#[derive(Debug, Default)]
pub struct Interner {
    ids: HashMap<EntityUri, Id>,
    uris: Vec<EntityUri>,
}

impl Interner {
    pub fn intern(&mut self, uri: &EntityUri) -> Id {
        if let Some(id) = self.ids.get(uri) {
            return *id;
        }
        let id = Id(u32::try_from(self.uris.len()).expect("fewer than 2^32 blocks"));
        self.ids.insert(uri.clone(), id);
        self.uris.push(uri.clone());
        id
    }

    pub fn uri(&self, id: Id) -> &EntityUri {
        &self.uris[id.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_keeps_its_id() {
        let mut interner = Interner::default();
        let (a, b) = (EntityUri::block("a"), EntityUri::block("b"));
        let id_a = interner.intern(&a);
        let id_b = interner.intern(&b);
        assert_ne!(id_a, id_b);
        assert_eq!(interner.intern(&a), id_a);
        assert_eq!(interner.uri(id_a), &a);
        assert_eq!(interner.uri(id_b), &b);
    }
}
