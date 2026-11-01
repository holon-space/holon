//! @c4 component
//! @c4 layer Core
//! Pattern: Interpreter
//!
//! Declared bidirectional text formats (string lenses). One YAML declaration
//! gives a parser, a printer that keeps unchanged parts of the original text
//! byte-for-byte, and a printer for new values. A format binds a TEXT column
//! to the `quantity` value type, defined here.

mod catalog;
mod cell;
mod format;
mod lang;
mod lens;
pub mod quantity;

pub use catalog::FieldFormat;
pub use catalog::bundled;
pub use catalog::bundled_names;
pub use cell::QUANTITY_MARKER_KEY;
pub use cell::QuantityCell;
pub use format::Format;
pub use format::FormatError;
pub use quantity::Quantity;
