//! A root of a non-map kind under the name of a root map, as a peer may write
//! one: Loro types a root by name and kind, so both roots coexist.

use loro::ContainerID;
use loro::ContainerType;
use loro::LoroDoc;
use loro::LoroResult;
use loro::LoroValue;
use loro::ValueOrContainer;
use proptest::prelude::Strategy;

/// What [`ForeignRootKind::value`] reads, for a crate that does not name
/// `loro`.
pub type ForeignRootValue = LoroValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignRootKind {
    List,
    MovableList,
    Text,
    Tree,
    Counter,
}

impl ForeignRootKind {
    pub const ALL: [Self; 5] = [
        Self::List,
        Self::MovableList,
        Self::Text,
        Self::Tree,
        Self::Counter,
    ];

    pub fn container_type(self) -> ContainerType {
        match self {
            Self::List => ContainerType::List,
            Self::MovableList => ContainerType::MovableList,
            Self::Text => ContainerType::Text,
            Self::Tree => ContainerType::Tree,
            Self::Counter => ContainerType::Counter,
        }
    }

    pub fn root_id(self, name: &str) -> ContainerID {
        ContainerID::new_root(name, self.container_type())
    }

    /// The value of the root `name` of this kind.
    pub fn value(self, doc: &LoroDoc, name: &str) -> LoroValue {
        let root = doc
            .get_container(self.root_id(name))
            .expect("a root id names a container");
        ValueOrContainer::Container(root).get_deep_value()
    }

    /// Writes one element into the root `name` of this kind, so name
    /// resolution sees a non-empty root.
    pub fn plant(self, doc: &LoroDoc, name: &str) -> LoroResult<()> {
        match self {
            Self::List => doc.get_list(name).push("foreign").map(drop),
            Self::MovableList => doc.get_movable_list(name).push("foreign").map(drop),
            Self::Text => doc.get_text(name).insert(0, "foreign"),
            Self::Tree => doc.get_tree(name).create(None).map(drop),
            Self::Counter => doc.get_counter(name).increment(1.0),
        }
    }

    /// Deletes every element of the root `name` of this kind, so name
    /// resolution passes over it unless it is a counter.
    pub fn empty(self, doc: &LoroDoc, name: &str) -> LoroResult<()> {
        match self {
            Self::List => {
                let list = doc.get_list(name);
                list.delete(0, list.len())
            }
            Self::MovableList => {
                let list = doc.get_movable_list(name);
                list.delete(0, list.len())
            }
            Self::Text => {
                let text = doc.get_text(name);
                text.delete(0, text.len_unicode())
            }
            Self::Tree => {
                let tree = doc.get_tree(name);
                tree.roots()
                    .into_iter()
                    .try_for_each(|node| tree.delete(node))
            }
            Self::Counter => {
                let counter = doc.get_counter(name);
                counter.decrement(counter.get_value())
            }
        }?;
        let value = self.value(doc, name);
        assert_eq!(
            value,
            self.container_type().default_value(),
            "{self:?} root emptied"
        );
        Ok(())
    }
}

pub fn foreign_root_kind() -> impl Strategy<Value = ForeignRootKind> {
    proptest::sample::select(ForeignRootKind::ALL.to_vec())
}
