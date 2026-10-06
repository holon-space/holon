//! The services a type declares under `services:` in its YAML. One generic
//! mechanism serves each service for every type that declares it.

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

use crate::computation::FieldIdent;
use crate::entity::FieldLifetime;
use crate::entity::FieldSchema;
use crate::entity::TypeDefinition;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Declared")]
pub struct TypeServices {
    /// The one-line label every service shows for an entity of this type.
    pub title: FieldIdent,
    /// Fields quick-open and `[[` autocomplete match against.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub searchable: Vec<FieldIdent>,
    /// An entity of this type can be a link target.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub linkable: bool,
    /// An entity of this type renders by URI: embed, navigate, focus root.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub embeddable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dense_view: Option<DenseView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<Hierarchy>,
    /// Fields that hold inline marks, so links are extracted from them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rich_text: Vec<FieldIdent>,
}

/// The MCP dense projection of an entity: `body` is its text, every other
/// field becomes a property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DenseView {
    pub body: FieldIdent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hierarchy {
    pub parent: FieldIdent,
    /// Absent: sibling order is kept by the type's home, not in a column
    /// (block's fractional index).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<FieldIdent>,
}

/// [`TypeServices`] as written, field references still raw so a refusal can
/// name the key that holds one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declared {
    title: String,
    #[serde(default)]
    searchable: Slot<Vec<String>>,
    #[serde(default)]
    linkable: bool,
    #[serde(default)]
    embeddable: bool,
    #[serde(default)]
    dense_view: Slot<DeclaredDenseView>,
    #[serde(default)]
    hierarchy: Slot<DeclaredHierarchy>,
    #[serde(default)]
    rich_text: Slot<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredDenseView {
    body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredHierarchy {
    parent: String,
    #[serde(default)]
    order: Slot<String>,
}

/// An optional key: absent, present with no value (a YAML null, which is
/// what a mis-indented body leaves behind), or given.
enum Slot<T> {
    Absent,
    Empty,
    Given(T),
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot::Absent
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Slot<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<T>::deserialize(d)?.map_or(Slot::Empty, Slot::Given))
    }
}

impl<T> Slot<T> {
    fn given(self, key: &str) -> Result<Option<T>, String> {
        match self {
            Slot::Absent => Ok(None),
            Slot::Empty => Err(format!(
                "`{key}` is present with no value; indent its body under it"
            )),
            Slot::Given(value) => Ok(Some(value)),
        }
    }
}

/// `TypeDefinition.services`: a `services:` key with no body is refused, an
/// absent one is `None`.
pub(crate) fn declared_services<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<TypeServices>, D::Error> {
    Slot::deserialize(d)?
        .given("services")
        .map_err(serde::de::Error::custom)
}

fn field_ref(key: &str, name: String) -> Result<FieldIdent, String> {
    FieldIdent::try_from(name).map_err(|e| format!("services.{key}: {e}"))
}

fn field_refs(key: &str, slot: Slot<Vec<String>>) -> Result<Vec<FieldIdent>, String> {
    slot.given(&format!("services.{key}"))?
        .unwrap_or_default()
        .into_iter()
        .map(|name| field_ref(key, name))
        .collect()
}

impl TryFrom<Declared> for TypeServices {
    type Error = String;

    fn try_from(declared: Declared) -> Result<Self, String> {
        let dense_view = match declared.dense_view.given("services.dense_view")? {
            Some(d) => Some(DenseView {
                body: field_ref("dense_view.body", d.body)?,
            }),
            None => None,
        };
        let hierarchy = match declared.hierarchy.given("services.hierarchy")? {
            Some(h) => Some(Hierarchy {
                parent: field_ref("hierarchy.parent", h.parent)?,
                order: match h.order.given("services.hierarchy.order")? {
                    Some(order) => Some(field_ref("hierarchy.order", order)?),
                    None => None,
                },
            }),
            None => None,
        };
        Ok(TypeServices {
            title: field_ref("title", declared.title)?,
            searchable: field_refs("searchable", declared.searchable)?,
            linkable: declared.linkable,
            embeddable: declared.embeddable,
            dense_view,
            hierarchy,
            rich_text: field_refs("rich_text", declared.rich_text)?,
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ServiceFieldError {
    #[error("type '{type_name}': services.{service} names `{field}`, {reason}")]
    Unusable {
        type_name: String,
        service: &'static str,
        field: String,
        reason: UnusableField,
    },
    #[error(
        "type '{type_name}': services.{service} names `{field}`, which is {sql_type}; the service reads TEXT"
    )]
    NotText {
        type_name: String,
        service: &'static str,
        field: String,
        sql_type: String,
    },
    #[error(
        "type '{type_name}': services.hierarchy.parent names `{field}`, the type's own primary key; an entity cannot be its own parent"
    )]
    SelfParent { type_name: String, field: String },
    #[error(
        "type '{type_name}': services.hierarchy.order names `{field}`, which is {sql_type}; sibling order reads INTEGER or REAL"
    )]
    NotNumeric {
        type_name: String,
        field: String,
        sql_type: String,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UnusableField {
    #[error("which is not a field of the type")]
    Missing,
    #[error("which the engine owns; a service reads a field the row's author writes")]
    EngineOwned,
    #[error("which is computed; a service reads a column of the type's stored row")]
    Computed,
    #[error("which is transient; a service reads a column of the type's stored row")]
    Transient,
    #[error("which is historical; a service reads a column of the type's stored row")]
    Historical,
}

impl TypeServices {
    /// Every field the declaration names is a persisted field of `type_def`
    /// that its rows' author writes, of the SQL type its service reads.
    pub fn check_fields(&self, type_def: &TypeDefinition) -> Result<(), ServiceFieldError> {
        let text = std::iter::once(("title", &self.title))
            .chain(self.searchable.iter().map(|f| ("searchable", f)))
            .chain(self.rich_text.iter().map(|f| ("rich_text", f)))
            .chain(self.dense_view.iter().map(|d| ("dense_view.body", &d.body)));
        for (service, field) in text {
            let schema = authored_field(type_def, service, field)?;
            if !schema.sql_type.eq_ignore_ascii_case("TEXT") {
                return Err(ServiceFieldError::NotText {
                    type_name: type_def.name.clone(),
                    service,
                    field: field.to_string(),
                    sql_type: schema.sql_type.clone(),
                });
            }
        }
        if let Some(hierarchy) = &self.hierarchy {
            authored_field(type_def, "hierarchy.parent", &hierarchy.parent)?;
            if hierarchy.parent == type_def.primary_key {
                return Err(ServiceFieldError::SelfParent {
                    type_name: type_def.name.clone(),
                    field: hierarchy.parent.to_string(),
                });
            }
            if let Some(order) = &hierarchy.order {
                let schema = authored_field(type_def, "hierarchy.order", order)?;
                if !["INTEGER", "REAL"]
                    .iter()
                    .any(|t| schema.sql_type.eq_ignore_ascii_case(t))
                {
                    return Err(ServiceFieldError::NotNumeric {
                        type_name: type_def.name.clone(),
                        field: order.to_string(),
                        sql_type: schema.sql_type.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

fn authored_field<'a>(
    type_def: &'a TypeDefinition,
    service: &'static str,
    field: &FieldIdent,
) -> Result<&'a FieldSchema, ServiceFieldError> {
    let schema = type_def.fields.iter().find(|s| s.name == field.as_str());
    let reason = match schema {
        None => UnusableField::Missing,
        Some(s) if s.value_kind.is_engine_owned() => UnusableField::EngineOwned,
        Some(s) => match s.lifetime {
            FieldLifetime::Persistent => return Ok(s),
            FieldLifetime::Computed { .. } => UnusableField::Computed,
            FieldLifetime::Transient => UnusableField::Transient,
            FieldLifetime::Historical => UnusableField::Historical,
        },
    };
    Err(ServiceFieldError::Unusable {
        type_name: type_def.name.clone(),
        service,
        field: field.to_string(),
        reason,
    })
}
