//! The formats shipped with Holon, compiled once on first use.

use std::sync::Arc;
use std::sync::OnceLock;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

use crate::format::Format;
use crate::format::FormatError;

/// (name, declaration). A declaration's `name:` must equal its catalog name.
const BUNDLED: [(&str, &str); 2] = [
    ("tsl_count", include_str!("../formats/tsl_count.yaml")),
    (
        "cooklang_quantity",
        include_str!("../formats/cooklang_quantity.yaml"),
    ),
];

static COMPILED: [OnceLock<Result<Arc<Format>, FormatError>>; BUNDLED.len()] =
    [const { OnceLock::new() }; BUNDLED.len()];

pub fn bundled_names() -> impl Iterator<Item = &'static str> {
    BUNDLED.iter().map(|(name, _)| *name)
}

pub fn bundled(name: &str) -> Result<Arc<Format>, FormatError> {
    let Some(i) = BUNDLED.iter().position(|(n, _)| *n == name) else {
        return Err(FormatError::Unknown {
            name: name.to_string(),
            known: bundled_names().collect::<Vec<_>>().join(", "),
        });
    };
    COMPILED[i]
        .get_or_init(|| {
            let format = Format::from_yaml(BUNDLED[i].1)?;
            if format.name() != name {
                return Err(FormatError::Declaration {
                    name: name.to_string(),
                    detail: format!("the file declares `name: {}`", format.name()),
                });
            }
            if name.to_ascii_uppercase().contains("INT") {
                return Err(FormatError::Declaration {
                    name: name.to_string(),
                    detail: format!(
                        "the column type `{DECLARED_TYPE_PREFIX}{name}` contains INT, which gives \
                         the column INTEGER affinity and rewrites numeric-looking text"
                    ),
                });
            }
            Ok(Arc::new(format))
        })
        .clone()
}

/// A field's text format, as declared by `format: <name>` in a type YAML.
/// Deserializing compiles the format, so an unknown or ill-formed one is a
/// load error. The column stays TEXT; a [`crate::QuantityCell`] is the typed
/// view of one of its cells.
#[derive(Clone)]
pub struct FieldFormat(Arc<Format>);

/// The SQL column type `TEXT_FMT_<name>` declares format `<name>`. It
/// contains TEXT, so the column keeps TEXT affinity.
const DECLARED_TYPE_PREFIX: &str = "TEXT_FMT_";

impl FieldFormat {
    pub fn named(name: &str) -> Result<FieldFormat, FormatError> {
        bundled(name).map(FieldFormat)
    }

    /// The SQL column type that declares this format.
    pub fn declared_type(&self) -> String {
        format!("{DECLARED_TYPE_PREFIX}{}", self.0.name())
    }

    /// The format a column's declared SQL type names, `None` when it names
    /// none.
    pub fn of_declared_type(declared: &str) -> Result<Option<FieldFormat>, FormatError> {
        match declared.get(..DECLARED_TYPE_PREFIX.len()) {
            Some(prefix) if prefix.eq_ignore_ascii_case(DECLARED_TYPE_PREFIX) => {
                FieldFormat::named(&declared[DECLARED_TYPE_PREFIX.len()..].to_ascii_lowercase())
                    .map(Some)
            }
            _ => Ok(None),
        }
    }

    pub fn format(&self) -> &Format {
        &self.0
    }
}

/// Bundled formats are unique by name.
impl PartialEq for FieldFormat {
    fn eq(&self, other: &Self) -> bool {
        self.0.name() == other.0.name()
    }
}

impl std::fmt::Debug for FieldFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FieldFormat({})", self.0.name())
    }
}

impl Serialize for FieldFormat {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0.name())
    }
}

impl<'de> Deserialize<'de> for FieldFormat {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        FieldFormat::named(&name).map_err(serde::de::Error::custom)
    }
}
