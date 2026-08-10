//! The declared field vocabulary of an entity — the ONE source the arc places,
//! the intent-write vocabulary, the projection column map and the `block_raw`
//! column list all derive from.
//!
//! Why the declaration lives in this leaf crate and not next to
//! `holon_api::TypeDefinition`: `holon-macros` parses arc places at expansion
//! time, so the vocabulary must be reachable from a proc-macro crate, and
//! `holon-api` sits ABOVE `holon-macros`. `TypeDefinition` stays the runtime
//! schema of a dynamically created entity; [`SchemaSource`] is the seam that
//! lets both kinds answer the same question, so a place check never has to
//! know which kind it is looking at.
//!
//! Intended future callers (they generalize on this same source rather than
//! growing a second one): `crate::pattern::Subject`, whose variants are the
//! same relation vocabulary, and the guard compiler's `SchemaAbstraction`,
//! whose column map is already spelled with the [`block`] / [`clock`] name
//! constants below.

/// Where a field's data lives. Only [`FieldStorage::Column`] fields are columns
/// of the entity's own table, which is what the DDL lock compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldStorage {
    /// A column of the entity's own table.
    Column,
    /// A junction-backed edge set, hydrated into the read matview.
    EdgeSet,
    /// Carried inside the `properties` JSON column, not a column of its own.
    Property,
    /// Stored nowhere. Named so an operation can declare it, or so the intent
    /// boundary can refuse it, rather than letting it fall through unnoticed.
    Unstored,
}

/// How a `set_field` intent may treat the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldIntent {
    /// Intent may write it, and the intent vocabulary carries a named variant
    /// for it.
    Writable,
    /// An order key. Minted by the ordering authority only (Model.md invariant
    /// 3); intent expresses a move positionally.
    OrderKey,
    /// Storage bookkeeping or derived state, written by the storage layer.
    StorageInternal,
    /// The intent boundary has no named variant for it — a write naming it is
    /// an ordinary user property, and the field's own writer owns the real
    /// column or junction.
    Unnamed,
}

/// One declared field of an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaField {
    pub name: &'static str,
    pub storage: FieldStorage,
    pub intent: FieldIntent,
    /// Whether `#[reads]` / `#[emits]` may name it. A field that is pure
    /// storage bookkeeping is not a place an operation can declare.
    pub arc_place: bool,
}

/// An entity's declared field vocabulary.
#[derive(Debug, Clone, Copy)]
pub struct EntitySchema {
    pub relation: &'static str,
    pub fields: &'static [SchemaField],
}

impl EntitySchema {
    pub fn field(&self, name: &str) -> Option<&'static SchemaField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Every field an arc may name — the parser's vocabulary.
    pub fn arc_places(&self) -> Vec<&'static str> {
        self.fields
            .iter()
            .filter(|f| f.arc_place)
            .map(|f| f.name)
            .collect()
    }

    /// Every field backed by a column of the entity's own table, in
    /// declaration order. The DDL lock compares exactly this set.
    pub fn columns(&self) -> Vec<&'static str> {
        self.select(FieldStorage::Column)
    }

    /// Every junction-backed edge field.
    pub fn edge_sets(&self) -> Vec<&'static str> {
        self.select(FieldStorage::EdgeSet)
    }

    /// Every field a `set_field` intent may write under a named variant.
    pub fn intent_writable(&self) -> Vec<&'static str> {
        self.fields
            .iter()
            .filter(|f| f.intent == FieldIntent::Writable)
            .map(|f| f.name)
            .collect()
    }

    fn select(&self, storage: FieldStorage) -> Vec<&'static str> {
        self.fields
            .iter()
            .filter(|f| f.storage == storage)
            .map(|f| f.name)
            .collect()
    }
}

/// The `block` entity's field names. Spelled as constants so a rename lands in
/// one place and the projection column map, the DDL lock and the arc
/// vocabulary all move with it.
pub mod block {
    pub const RELATION: &str = "block";
    pub const ID: &str = "id";
    pub const PARENT_ID: &str = "parent_id";
    pub const SORT_KEY: &str = "sort_key";
    pub const AFTER_BLOCK_ID: &str = "after_block_id";
    pub const CONTENT: &str = "content";
    pub const CONTENT_TYPE: &str = "content_type";
    pub const SOURCE_LANGUAGE: &str = "source_language";
    pub const SOURCE_NAME: &str = "source_name";
    pub const PROPERTIES: &str = "properties";
    pub const MARKS: &str = "marks";
    pub const COLLAPSED: &str = "collapsed";
    pub const WIDGET_ONLY: &str = "widget_only";
    pub const COMPLETED: &str = "completed";
    pub const BLOCK_TYPE: &str = "block_type";
    pub const CREATED_AT: &str = "created_at";
    pub const UPDATED_AT: &str = "updated_at";
    pub const CHANGE_ORIGIN: &str = "_change_origin";
    pub const WRITE_SEQ: &str = "write_seq";
    pub const TASK_STATE: &str = "task_state";
    pub const TAGS: &str = "tags";
    pub const REQUIRES: &str = "requires";
    pub const ADVICE_SUPPRESSED: &str = "advice_suppressed";
    pub const DEPTH: &str = "depth";
}

/// The `clock` relation's field names (ADR 0024 P5).
pub mod clock {
    pub const RELATION: &str = "clock";
    pub const GRAIN: &str = "grain";
    pub const TODAY: &str = "today";
    pub const EPOCH_DAY: &str = "epoch_day";
    pub const UPDATED_AT: &str = "updated_at";
}

const fn column(name: &'static str, intent: FieldIntent, arc_place: bool) -> SchemaField {
    SchemaField {
        name,
        storage: FieldStorage::Column,
        intent,
        arc_place,
    }
}

/// The `block` entity. Column order is the `block_raw` DDL order, so the lock
/// between the two reads as one list.
pub const BLOCK: EntitySchema = EntitySchema {
    relation: block::RELATION,
    fields: &[
        column(block::ID, FieldIntent::StorageInternal, true),
        column(block::PARENT_ID, FieldIntent::Writable, true),
        column(block::SORT_KEY, FieldIntent::OrderKey, true),
        column(block::CONTENT, FieldIntent::Writable, true),
        column(block::CONTENT_TYPE, FieldIntent::Writable, true),
        column(block::SOURCE_LANGUAGE, FieldIntent::Writable, true),
        column(block::SOURCE_NAME, FieldIntent::Writable, true),
        column(block::PROPERTIES, FieldIntent::Writable, true),
        column(block::MARKS, FieldIntent::Writable, true),
        column(block::COLLAPSED, FieldIntent::Writable, true),
        column(block::WIDGET_ONLY, FieldIntent::Writable, true),
        column(block::COMPLETED, FieldIntent::Writable, true),
        column(block::BLOCK_TYPE, FieldIntent::Writable, true),
        column(block::CREATED_AT, FieldIntent::StorageInternal, false),
        column(block::UPDATED_AT, FieldIntent::StorageInternal, false),
        column(block::CHANGE_ORIGIN, FieldIntent::StorageInternal, false),
        column(block::WRITE_SEQ, FieldIntent::StorageInternal, false),
        SchemaField {
            name: block::TASK_STATE,
            storage: FieldStorage::Property,
            intent: FieldIntent::Writable,
            arc_place: true,
        },
        SchemaField {
            name: block::TAGS,
            storage: FieldStorage::EdgeSet,
            intent: FieldIntent::Writable,
            arc_place: true,
        },
        SchemaField {
            name: block::REQUIRES,
            storage: FieldStorage::EdgeSet,
            intent: FieldIntent::Unnamed,
            arc_place: true,
        },
        SchemaField {
            name: block::ADVICE_SUPPRESSED,
            storage: FieldStorage::EdgeSet,
            intent: FieldIntent::Unnamed,
            arc_place: true,
        },
        // A positional anchor, never a stored value: an operation names it only
        // to declare it excluded.
        SchemaField {
            name: block::AFTER_BLOCK_ID,
            storage: FieldStorage::Unstored,
            intent: FieldIntent::OrderKey,
            arc_place: true,
        },
        // Tree depth is derived on read. Declared so a `set_field` naming it
        // fails loud instead of landing in `properties` as a user key.
        SchemaField {
            name: block::DEPTH,
            storage: FieldStorage::Unstored,
            intent: FieldIntent::StorageInternal,
            arc_place: false,
        },
    ],
};

/// The `clock` relation. One row per grain; `today` is the grain LABEL and
/// `epoch_day` the grain TICK.
pub const CLOCK: EntitySchema = EntitySchema {
    relation: clock::RELATION,
    fields: &[
        column(clock::GRAIN, FieldIntent::Unnamed, true),
        column(clock::TODAY, FieldIntent::Unnamed, true),
        column(clock::EPOCH_DAY, FieldIntent::Unnamed, false),
        column(clock::UPDATED_AT, FieldIntent::StorageInternal, false),
    ],
};

/// Every entity whose schema is declared in-tree. A relation outside this list
/// exists only at runtime and answers through a [`SchemaSource`] built from its
/// `TypeDefinition`.
pub const BUILTIN_SCHEMAS: &[&EntitySchema] = &[&BLOCK, &CLOCK];

/// Answers "does this relation exist, and does it have this field" for one
/// population of entities.
///
/// Two implementations by BINDING TIME: [`BuiltinSchemas`] answers at macro
/// expansion, where an unknown place is a compile error; an adapter over a
/// runtime `TypeDefinition` answers at registration, where an unknown place
/// refuses the registration.
pub trait SchemaSource {
    /// The places `relation` admits, or `None` when this source does not know
    /// the relation at all.
    fn arc_places(&self, relation: &str) -> Option<Vec<String>>;

    /// Every relation this source knows — the "known relations are …" half of
    /// a refusal.
    fn relations(&self) -> Vec<String>;
}

/// The in-tree declarations ([`BUILTIN_SCHEMAS`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltinSchemas;

impl SchemaSource for BuiltinSchemas {
    fn arc_places(&self, relation: &str) -> Option<Vec<String>> {
        BUILTIN_SCHEMAS
            .iter()
            .find(|s| s.relation == relation)
            .map(|s| s.arc_places().into_iter().map(str::to_string).collect())
    }

    fn relations(&self) -> Vec<String> {
        BUILTIN_SCHEMAS
            .iter()
            .map(|s| s.relation.to_string())
            .collect()
    }
}

/// Several sources consulted in order — the built-ins plus whatever entity
/// types exist at runtime. First source that knows the relation answers.
pub struct SchemaSources<'a>(pub Vec<&'a dyn SchemaSource>);

impl SchemaSource for SchemaSources<'_> {
    fn arc_places(&self, relation: &str) -> Option<Vec<String>> {
        self.0.iter().find_map(|s| s.arc_places(relation))
    }

    fn relations(&self) -> Vec<String> {
        let mut all: Vec<String> = self.0.iter().flat_map(|s| s.relations()).collect();
        all.sort();
        all.dedup();
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two fields with the same name would make `field()` answer with whichever
    /// came first and silently shadow the other's storage and intent.
    #[test]
    fn every_declared_field_name_is_unique() {
        for schema in BUILTIN_SCHEMAS {
            let mut names: Vec<&str> = schema.fields.iter().map(|f| f.name).collect();
            let count = names.len();
            names.sort_unstable();
            names.dedup();
            assert_eq!(
                count,
                names.len(),
                "{} has a duplicate field",
                schema.relation
            );
        }
    }

    /// An intent-writable field nobody can declare an arc for would make an op
    /// unable to declare a write it can perform.
    #[test]
    fn every_intent_writable_field_is_an_arc_place() {
        for name in BLOCK.intent_writable() {
            assert!(
                BLOCK.field(name).expect("declared").arc_place,
                "block.{name} is intent-writable but not an arc place"
            );
        }
    }

    #[test]
    fn the_builtin_source_answers_for_the_declared_relations_only() {
        assert_eq!(
            BuiltinSchemas.relations(),
            vec!["block".to_string(), "clock".to_string()]
        );
        assert!(BuiltinSchemas.arc_places("block").is_some());
        assert!(BuiltinSchemas.arc_places("todoist_task").is_none());
    }
}
