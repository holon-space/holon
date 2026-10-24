//! The write authority of one free-standing type in a Loro mode: its entities
//! live in the Global doc, one mergeable child map per entity under the root
//! map `entity:<canonical type name>`, one key per column, the overflow bag as
//! the mergeable child `properties`.
//!
//! The ops, their parameters, refusals and inverses are those of the SQL
//! authority (`SqlOperationProvider::for_type`), so a caller does not change
//! per mode. The Turso rows of these entities are the projector's to write.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::TargetScope;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_core::FieldDelta;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;
use holon_core::type_write_rule::CrudDeclarations;
use holon_core::type_write_rule::WriteSchema;
use loro::LoroDoc;
use loro::LoroMap;
use loro::LoroValue;
use loro::ValueOrContainer;

use crate::loro_backend::decode_property_json;
use crate::loro_backend::encode_property_value;
use crate::loro_document::LoroDocument;
use crate::write_origin::WriteOrigin;

/// The prefix of a type's root map in the Global doc.
pub const ENTITY_ROOT_PREFIX: &str = "entity:";

/// The store's own tombstone key, for a type that declares no `soft_delete`.
pub const TOMBSTONE_KEY: &str = "_deleted";

/// The name of `entity`'s root map.
pub fn entity_root_name(entity: &EntityName) -> String {
    format!("{ENTITY_ROOT_PREFIX}{}", entity.as_str())
}

pub struct LoroEntityOperations {
    doc: Arc<LoroDocument>,
    entity: EntityName,
    schema: WriteSchema,
}

/// One entity's stored state.
pub struct StoredEntity {
    /// `false` once the store's own tombstone is set.
    pub live: bool,
    /// Every field the entity holds, the id included, the overflow bag lifted
    /// to top-level keys.
    pub fields: StorageEntity,
}

impl LoroEntityOperations {
    /// Refuses a type this store cannot lower yet: one with edge fields
    /// (junction tables) or a hierarchical one (`id_references`).
    pub fn for_type(doc: Arc<LoroDocument>, type_def: &TypeDefinition) -> Result<Self> {
        if let Some(field) = type_def.fields.iter().find(|f| f.edge_name.is_some()) {
            return Err(format!(
                "type '{}': field '{}' is an edge field, which the Loro entity store cannot hold \
                 yet",
                type_def.name, field.name
            )
            .into());
        }
        if let Some(parent) = &type_def.id_references {
            return Err(format!(
                "type '{}' is hierarchical (its id references '{parent}'), which the Loro entity \
                 store cannot hold yet",
                type_def.name
            )
            .into());
        }
        Ok(Self {
            doc,
            entity: EntityName::new(type_def.name.clone()),
            schema: WriteSchema::from_type_def(type_def)?,
        })
    }

    fn store(&self) -> String {
        entity_root_name(&self.entity)
    }

    /// The store key of a schemed id of this type.
    fn key_of<'a>(&self, op: &str, id: &'a str) -> Result<&'a str> {
        let uri = EntityUri::schemed(id).ok_or_else(|| {
            format!(
                "{entity}/{op}: the id {id:?} carries no scheme. An entity reference names its \
                 entity (e.g. \"{entity}:{id}\")",
                entity = self.entity
            )
        })?;
        if uri.scheme() != self.entity.as_str() {
            return Err(format!(
                "{entity}/{op}: the id {id:?} names the entity '{}', not '{entity}'",
                uri.scheme(),
                entity = self.entity
            )
            .into());
        }
        Ok(&id[id.len() - uri.id().len()..])
    }

    pub fn read(&self, id: &str) -> Result<Option<StoredEntity>> {
        let key = self.key_of("read", id)?;
        let root = self.store();
        Ok(self
            .doc
            .with_read(|doc| Ok(read_entity(doc, &root, &self.entity, key)))?)
    }

    fn read_live(&self, op: &str, id: &str) -> Result<StorageEntity> {
        match self.read(id)? {
            Some(StoredEntity { live: true, fields }) => Ok(fields),
            Some(StoredEntity { live: false, .. }) => {
                Err(format!("{}/{op}: '{id}' is deleted", self.entity).into())
            }
            None => Err(format!("{}/{op}: no entity '{id}'", self.entity).into()),
        }
    }

    fn write(&self, f: impl FnOnce(&LoroMap) -> anyhow::Result<()>, id: &str) -> Result<()> {
        let key = self.key_of("write", id)?;
        let root = self.store();
        self.doc
            .with_write(WriteOrigin::BlockOps, |txn| {
                let entity = crate::mergeable_child::ensure_map(&txn.get_map(root.as_str()), key)?;
                f(&entity)
            })
            .map_err(|e| format!("{}: writing '{id}': {e:#}", self.entity).into())
    }

    fn id_param<'a>(op: &str, params: &'a StorageEntity) -> Result<&'a str> {
        params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("{op}: missing 'id' parameter").into())
    }

    fn partition(&self, params: &StorageEntity) -> Result<Vec<(String, Value)>> {
        let parted =
            self.schema
                .partition(self.entity.as_str(), &self.store(), params, |_, _| {
                    Ok(false)
                })?;
        let mut fields: Vec<(String, Value)> = parted
            .columns
            .into_iter()
            .filter(|(k, _)| k != "id")
            .collect();
        fields.extend(parted.overflow);
        Ok(fields)
    }

    fn put(&self, entity: &LoroMap, field: &str, value: &Value) -> anyhow::Result<()> {
        if self.schema.is_column(field) {
            entity.insert(field, column_to_loro(field, value)?)?;
            return Ok(());
        }
        let bag = crate::mergeable_child::ensure_map(entity, WriteSchema::OVERFLOW_COLUMN)?;
        if value.is_removed() {
            bag.delete(field)?;
        } else {
            bag.insert(field, encode_property_value(field, value)?)?;
        }
        Ok(())
    }

    fn set_field(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = Self::id_param("set_field", params)?;
        let field = params
            .get("field")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "set_field: missing 'field' parameter".to_string())?;
        let value = params
            .get("value")
            .ok_or_else(|| "set_field: missing 'value' parameter".to_string())?;
        self.schema
            .require_write_route(self.entity.as_str(), &self.store(), field)?;
        value.reject_reserved_marker(field)?;
        value.reject_non_finite_float(field)?;
        let is_column = self.schema.is_column(field);
        if value.is_removed() && is_column {
            return Err(format!(
                "set_field('{field}'): a column cannot take the removal sentinel — write \
                 Value::Null to clear it"
            )
            .into());
        }
        let held = self.read_live("set_field", id)?;
        let old = match held.get(field) {
            Some(v) => v.clone(),
            None if is_column => Value::Null,
            None => Value::REMOVED,
        };
        self.write(|entity| self.put(entity, field, value), id)?;
        let changes = if is_column {
            vec![FieldDelta::new(id, field, old.clone(), value.clone())]
        } else {
            Vec::new()
        };
        Ok(OperationResult::new(
            changes,
            self.op(
                "set_field",
                [
                    ("id".to_string(), Value::String(id.to_string())),
                    ("field".to_string(), Value::String(field.to_string())),
                    ("value".to_string(), old),
                ],
            ),
        ))
    }

    fn create(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = Self::id_param("create", params)?;
        let fields = self.partition(params)?;
        match self.read(id)? {
            Some(StoredEntity {
                live: true,
                fields: held,
            }) => {
                self.recognize(id, &held, params)?;
                self.write(
                    |entity| {
                        for (field, value) in &fields {
                            self.put(entity, field, value)?;
                        }
                        Ok(())
                    },
                    id,
                )?;
                Ok(OperationResult::irreversible(Vec::new()))
            }
            // A tombstoned or purged key keeps its child container, so a create
            // there clears every key it does not write.
            _ => {
                self.write(
                    |entity| {
                        let written: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
                        for stale in entity.keys().map(|k| k.to_string()).collect::<Vec<_>>() {
                            if stale != WriteSchema::OVERFLOW_COLUMN
                                && !written.contains(&stale.as_str())
                            {
                                entity.delete(&stale)?;
                            }
                        }
                        if let Some(ValueOrContainer::Container(loro::Container::Map(bag))) =
                            entity.get(WriteSchema::OVERFLOW_COLUMN)
                        {
                            for stale in bag.keys().map(|k| k.to_string()).collect::<Vec<_>>() {
                                if !written.contains(&stale.as_str()) {
                                    bag.delete(&stale)?;
                                }
                            }
                        }
                        for (field, value) in &fields {
                            self.put(entity, field, value)?;
                        }
                        Ok(())
                    },
                    id,
                )?;
                Ok(OperationResult::new(
                    vec![FieldDelta::new(
                        id,
                        "id",
                        Value::Null,
                        Value::String(id.to_string()),
                    )],
                    self.op(
                        "delete",
                        [("id".to_string(), Value::String(id.to_string()))],
                    ),
                ))
            }
        }
    }

    /// ADR 0029 D2: a create on a held id must name the same entity.
    fn recognize(&self, id: &str, held: &StorageEntity, params: &StorageEntity) -> Result<()> {
        let held_title = held.get("content").and_then(|v| v.as_string());
        let Some(requested) = params.get("content").and_then(|v| v.as_string()) else {
            return Err(format!(
                "create for {id}: that id is ALREADY HELD (holder title {held_title:?}) and this \
                 create carries no title, so there is nothing to recognize the holder by. \
                 Refusing rather than upserting over it — name the entity this create means, or \
                 address the existing entity with an `update`."
            )
            .into());
        };
        let uri = EntityUri::from_raw_for(self.entity.as_str(), id);
        match holon_api::identity_recognition::recognize_derived_id(&uri, held_title, requested) {
            holon_api::identity_recognition::Recognition::AlreadySatisfied => Ok(()),
            holon_api::identity_recognition::Recognition::UnnamedPlaceholder
            | holon_api::identity_recognition::Recognition::Free => {
                tracing::warn!(
                    id = %id,
                    requested_title = %requested,
                    held_title = ?held_title,
                    "adopting an UNNAMED placeholder entity at a held id — the create completes it \
                     with the requested title; no named holder is clobbered"
                );
                Ok(())
            }
            holon_api::identity_recognition::Recognition::Collision(c) => Err(Box::new(c)),
        }
    }

    fn update(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = Self::id_param("update", params)?;
        let fields = self.partition(params)?;
        self.read_live("update", id)?;
        self.write(
            |entity| {
                for (field, value) in &fields {
                    self.put(entity, field, value)?;
                }
                Ok(())
            },
            id,
        )?;
        Ok(OperationResult::irreversible(Vec::new()))
    }

    fn delete(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = Self::id_param("delete", params)?;
        let held = self.read_live("delete", id)?;
        let at = Value::String(chrono::Utc::now().to_rfc3339());
        let (key, change) = match self.schema.tombstone_column() {
            Some(column) => (
                column.to_string(),
                FieldDelta::new(
                    id,
                    column,
                    held.get(column).cloned().unwrap_or(Value::Null),
                    at.clone(),
                ),
            ),
            // The Turso row of a type without `soft_delete` is removed.
            None => (
                TOMBSTONE_KEY.to_string(),
                FieldDelta::new(id, "id", Value::String(id.to_string()), Value::Null),
            ),
        };
        self.write(
            |entity| {
                entity.insert(&key, column_to_loro(&key, &at)?)?;
                Ok(())
            },
            id,
        )?;
        Ok(OperationResult::new(vec![change], self.recreate(held)))
    }

    fn purge(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = Self::id_param("purge", params)?;
        let key = self.key_of("purge", id)?;
        let Some(held) = self.read(id)? else {
            return Ok(OperationResult::declared_irreversible(
                Vec::new(),
                "purge: target entity absent, nothing to restore",
            ));
        };
        let root = self.store();
        self.doc
            .with_write(WriteOrigin::BlockOps, |txn| {
                txn.get_map(root.as_str()).delete(key)?;
                Ok(())
            })
            .map_err(|e| format!("{}: purging '{id}': {e:#}", self.entity))?;
        let change = FieldDelta::new(id, "id", Value::String(id.to_string()), Value::Null);
        Ok(OperationResult::new(
            vec![change],
            self.recreate(held.fields),
        ))
    }

    fn recreate(&self, mut fields: StorageEntity) -> Operation {
        fields.remove(TOMBSTONE_KEY);
        self.op(
            "create",
            fields.into_iter().map(|(k, v)| (k.to_string(), v)),
        )
    }

    fn op(&self, name: &str, params: impl IntoIterator<Item = (String, Value)>) -> Operation {
        Operation::from_params(self.entity.clone(), name, name, params)
    }
}

/// Read one entity of the type whose root map is `root`, or `None` when the
/// key holds nothing.
pub fn read_entity(
    doc: &LoroDoc,
    root: &str,
    entity: &EntityName,
    key: &str,
) -> Option<StoredEntity> {
    let map = match doc.get_map(root).get(key)? {
        ValueOrContainer::Container(loro::Container::Map(m)) => m,
        other => panic!("{root}: entity key {key:?} holds {other:?}, not a map"),
    };
    let mut fields: StorageEntity = HashMap::new();
    let mut live = true;
    map.for_each(|field, value| match value {
        ValueOrContainer::Value(v) if field == TOMBSTONE_KEY => live = v.is_null(),
        ValueOrContainer::Value(v) => {
            fields.insert(Arc::from(field), column_from_loro(field, &v));
        }
        ValueOrContainer::Container(loro::Container::Map(bag))
            if field == WriteSchema::OVERFLOW_COLUMN =>
        {
            bag.for_each(|k, v| match v {
                ValueOrContainer::Value(LoroValue::String(json)) => {
                    fields.insert(Arc::from(k), decode_property_json(k, &json));
                }
                other => panic!("{root}/{key}: bag key {k:?} holds {other:?}, not a JSON string"),
            });
        }
        other => panic!("{root}/{key}: field {field:?} holds {other:?}"),
    });
    if fields.is_empty() && live {
        return None;
    }
    fields.insert(
        Arc::from("id"),
        Value::String(format!("{}:{key}", entity.as_str())),
    );
    Some(StoredEntity { live, fields })
}

fn column_to_loro(field: &str, value: &Value) -> anyhow::Result<LoroValue> {
    Ok(match value {
        Value::String(s) | Value::DateTime(s) | Value::Json(s) => LoroValue::from(s.as_str()),
        Value::Integer(i) => LoroValue::from(*i),
        Value::Float(f) => LoroValue::from(*f),
        Value::Boolean(b) => LoroValue::from(*b),
        Value::Null => LoroValue::Null,
        other => anyhow::bail!("column '{field}' cannot hold {other:?}"),
    })
}

fn column_from_loro(field: &str, value: &LoroValue) -> Value {
    match value {
        LoroValue::String(s) => Value::String(s.to_string()),
        LoroValue::I64(i) => Value::Integer(*i),
        LoroValue::Double(f) => Value::Float(*f),
        LoroValue::Bool(b) => Value::Boolean(*b),
        LoroValue::Null => Value::Null,
        other => panic!("column '{field}' holds {other:?}"),
    }
}

#[async_trait]
impl OperationProvider for LoroEntityOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        holon_core::type_write_rule::crud_descriptors(
            &self.entity,
            self.entity.as_str(),
            TargetScope::Global,
            &self.schema,
            CrudDeclarations::undeclared(),
        )
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        assert_eq!(
            entity_name, &self.entity,
            "LoroEntityOperations: expected entity '{}', got '{entity_name}'",
            self.entity
        );
        match op_name {
            "set_field" => self.set_field(&params),
            "create" => self.create(&params),
            "update" => self.update(&params),
            "delete" => self.delete(&params),
            "purge" => self.purge(&params),
            other => Err(format!("{}: unknown operation '{other}'", self.entity).into()),
        }
    }
}

/// The Loro store as the write authority of every free-standing type, except
/// an integration mirror: its system of record is outside Holon, so it keeps
/// the SQL authority over the table its connector syncs. `mirrors` is `None`
/// in a session without Turso, which holds no mirror table.
pub struct LoroTypeWriteAuthority {
    doc: Arc<LoroDocument>,
    mirrors: Option<Arc<dyn holon_core::TypeWriteAuthority>>,
}

impl LoroTypeWriteAuthority {
    pub fn new(
        doc: Arc<LoroDocument>,
        mirrors: Option<Arc<dyn holon_core::TypeWriteAuthority>>,
    ) -> Self {
        Self { doc, mirrors }
    }
}

impl LoroTypeWriteAuthority {
    /// The authority of an integration mirror type, or `None` for a type the
    /// Loro store holds.
    fn mirror_authority(
        &self,
        type_def: &TypeDefinition,
    ) -> Result<Option<&Arc<dyn holon_core::TypeWriteAuthority>>> {
        let Some(integration) = type_def.owning_integration() else {
            return Ok(None);
        };
        Ok(Some(self.mirrors.as_ref().ok_or_else(|| {
            format!(
                "type '{}' mirrors integration '{integration}', whose table needs Turso; \
                 this session has none",
                type_def.name
            )
        })?))
    }
}

#[async_trait]
impl holon_core::TypeWriteAuthority for LoroTypeWriteAuthority {
    fn for_type(&self, type_def: &TypeDefinition) -> Result<Arc<dyn OperationProvider>> {
        if let Some(mirrors) = self.mirror_authority(type_def)? {
            return mirrors.for_type(type_def);
        }
        Ok(Arc::new(LoroEntityOperations::for_type(
            self.doc.clone(),
            type_def,
        )?))
    }

    async fn read(&self, type_def: &TypeDefinition, id: &str) -> Result<Option<StorageEntity>> {
        if let Some(mirrors) = self.mirror_authority(type_def)? {
            return mirrors.read(type_def, id).await;
        }
        Ok(LoroEntityOperations::for_type(self.doc.clone(), type_def)?
            .read(id)?
            .filter(|stored| stored.live)
            .map(|stored| stored.fields))
    }

    async fn read_all(&self, type_def: &TypeDefinition) -> Result<Vec<StorageEntity>> {
        if let Some(mirrors) = self.mirror_authority(type_def)? {
            return mirrors.read_all(type_def).await;
        }
        let entity = EntityName::new(type_def.name.clone());
        let root = entity_root_name(&entity);
        Ok(self.doc.with_read(|doc| {
            let mut keys = Vec::new();
            doc.get_map(root.as_str())
                .for_each(|key, _| keys.push(key.to_string()));
            Ok(keys
                .iter()
                .filter_map(|key| read_entity(doc, &root, &entity, key))
                .filter(|stored| stored.live)
                .map(|stored| stored.fields)
                .collect())
        })?)
    }
}

/// The `(type root, store key)` pairs a `subscribe_root` event of the Global
/// doc names, read from the event alone: the callback must not touch the doc.
pub fn changed_entities(event: &loro::event::DiffEvent) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for cd in event.events.iter() {
        // A mergeable child is a root container under a synthetic name, so the
        // key comes from the path element whose PREDECESSOR is the type root.
        for pair in cd.path.windows(2) {
            if let (loro::ContainerID::Root { name, .. }, _) = &pair[0]
                && name.starts_with(ENTITY_ROOT_PREFIX)
                && let (_, loro::Index::Key(key)) = &pair[1]
            {
                out.insert((name.to_string(), key.to_string()));
            }
        }
        // A purge, or the insert of a child: the type root's own map diff.
        if let (loro::ContainerID::Root { name, .. }, loro::event::Diff::Map(delta)) =
            (cd.target, &cd.diff)
            && name.starts_with(ENTITY_ROOT_PREFIX)
        {
            for key in delta.updated.keys() {
                out.insert((name.to_string(), key.to_string()));
            }
        }
    }
    out
}

pub type ChangedRows = HashMap<EntityName, Vec<(String, Option<StorageEntity>)>>;

/// The current row of each changed entity, per type: `None` when the store
/// holds it no longer (purged, or under the store's own tombstone). A peer
/// names the type roots, so a root that names no entity refuses the batch.
pub fn read_changed_rows(
    doc: &LoroDoc,
    changed: &BTreeSet<(String, String)>,
) -> anyhow::Result<ChangedRows> {
    let mut out = ChangedRows::new();
    for (root, key) in changed {
        let raw = root
            .strip_prefix(ENTITY_ROOT_PREFIX)
            .expect("changed_entities yields type roots only");
        let entity = EntityName::parse_named(raw)
            .map_err(|e| anyhow::anyhow!("typed-entity root `{root}`: {e}"))?;
        let row = read_entity(doc, root, &entity, key)
            .filter(|stored| stored.live)
            .map(|stored| stored.fields);
        let uri = format!("{}:{key}", entity.as_str());
        out.entry(entity).or_default().push((uri, row));
    }
    Ok(out)
}

/// Where the projector lands typed-entity rows: the Turso table of the type.
#[async_trait]
pub trait TypedEntitySink: Send + Sync {
    /// Make `entity`'s table hold exactly `rows` for these ids: a row upserts,
    /// `None` removes it. Idempotent.
    async fn project(
        &self,
        entity: &EntityName,
        rows: Vec<(String, Option<StorageEntity>)>,
    ) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_that_names_no_entity_refuses_the_batch() {
        let changed = BTreeSet::from([(format!("{ENTITY_ROOT_PREFIX}bad name"), "k".to_string())]);
        let err = read_changed_rows(&LoroDoc::new(), &changed).unwrap_err();
        assert!(err.to_string().contains("`entity:bad name`"), "{err}");
        assert!(
            err.to_string().contains("is not a valid entity name"),
            "{err}"
        );
    }
}
