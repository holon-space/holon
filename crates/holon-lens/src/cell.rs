//! A formatted TEXT cell read as a quantity.

use serde::Deserialize;
use serde::Serialize;

use crate::catalog::FieldFormat;
use crate::format::FormatError;
use crate::quantity::Quantity;

/// The key carrying a [`QuantityCell`] on the wire. Only the engine writes it:
/// the write legs refuse authored values that carry it.
pub const QUANTITY_MARKER_KEY: &str = "__holon_quantity";

/// The text of a formatted cell and the quantity it denotes. The text is the
/// stored truth; the quantity is always parsed from it, also on deserialize.
#[derive(Clone, Debug, PartialEq)]
pub struct QuantityCell {
    format: FieldFormat,
    original: String,
    value: Option<Quantity>,
}

impl QuantityCell {
    pub fn parse(format: FieldFormat, original: String) -> Result<QuantityCell, FormatError> {
        let value = format.format().parse(&original)?;
        Ok(QuantityCell {
            format,
            original,
            value,
        })
    }

    /// A cell for `value` with the format's default spelling.
    pub fn print(
        format: FieldFormat,
        value: Option<&Quantity>,
    ) -> Result<QuantityCell, FormatError> {
        let text = format.format().print(value, None)?;
        QuantityCell::parse(format, text)
    }

    /// This cell with `value`, keeping the bytes of every chunk of the text
    /// whose value did not change. The new value is read back from the printed
    /// text, so a rounding format stores and reports the rounded value.
    pub fn with_value(&self, value: Option<&Quantity>) -> Result<QuantityCell, FormatError> {
        let text = self.format.format().print(value, Some(&self.original))?;
        QuantityCell::parse(self.format.clone(), text)
    }

    pub fn format(&self) -> &FieldFormat {
        &self.format
    }

    pub fn original(&self) -> &str {
        &self.original
    }

    /// `None` is the format's "no amount" text.
    pub fn value(&self) -> Option<&Quantity> {
        self.value.as_ref()
    }
}

#[derive(Serialize)]
struct WireRef<'a> {
    format: &'a FieldFormat,
    original: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireFields {
    format: FieldFormat,
    original: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    #[serde(rename = "__holon_quantity")]
    fields: WireFields,
}

impl Serialize for QuantityCell {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(1))?;
        map.serialize_entry(
            QUANTITY_MARKER_KEY,
            &WireRef {
                format: &self.format,
                original: &self.original,
            },
        )?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for QuantityCell {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Wire {
            fields: WireFields { format, original },
        } = Wire::deserialize(d)?;
        QuantityCell::parse(format, original).map_err(serde::de::Error::custom)
    }
}
