//! The services a type declares under `services:` in its YAML. One generic
//! mechanism serves each service for every type that declares it.

use serde::Deserialize;
use serde::Serialize;

use crate::computation::FieldIdent;
use crate::entity::TypeDefinition;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeServices {
    /// The one-line label every service shows for an entity of this type.
    pub title: FieldIdent,
    /// Fields quick-open and `[[` autocomplete match against.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub searchable: Vec<FieldIdent>,
    /// An entity of this type can be a link target.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linkable: bool,
    /// An entity of this type renders by URI: embed, navigate, focus root.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub embeddable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dense_view: Option<DenseView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<Hierarchy>,
    /// Fields that hold inline marks, so links are extracted from them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rich_text: Vec<FieldIdent>,
}

/// The MCP dense projection of an entity: `body` is its text, every other
/// field becomes a property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenseView {
    pub body: FieldIdent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hierarchy {
    pub parent: FieldIdent,
    /// Absent: sibling order is kept by the type's home, not in a column
    /// (block's fractional index).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<FieldIdent>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ServiceFieldError {
    #[error(
        "type '{type_name}': services.{service} names `{field}`, which is not a field of the type"
    )]
    UnknownField {
        type_name: String,
        service: &'static str,
        field: String,
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
}

impl TypeServices {
    /// Every field the declaration names exists on `type_def`, and the ones a
    /// service reads as text are TEXT.
    pub fn check_fields(&self, type_def: &TypeDefinition) -> Result<(), ServiceFieldError> {
        let text = std::iter::once(("title", &self.title))
            .chain(self.searchable.iter().map(|f| ("searchable", f)))
            .chain(self.rich_text.iter().map(|f| ("rich_text", f)))
            .chain(self.dense_view.iter().map(|d| ("dense_view.body", &d.body)));
        let any = self.hierarchy.iter().flat_map(|h| {
            std::iter::once(("hierarchy.parent", &h.parent))
                .chain(h.order.iter().map(|o| ("hierarchy.order", o)))
        });
        for (service, field, needs_text) in text
            .map(|(s, f)| (s, f, true))
            .chain(any.map(|(s, f)| (s, f, false)))
        {
            let Some(schema) = type_def.fields.iter().find(|s| s.name == field.as_str()) else {
                return Err(ServiceFieldError::UnknownField {
                    type_name: type_def.name.clone(),
                    service,
                    field: field.to_string(),
                });
            };
            if needs_text && !schema.sql_type.eq_ignore_ascii_case("TEXT") {
                return Err(ServiceFieldError::NotText {
                    type_name: type_def.name.clone(),
                    service,
                    field: field.to_string(),
                    sql_type: schema.sql_type.clone(),
                });
            }
        }
        Ok(())
    }
}
