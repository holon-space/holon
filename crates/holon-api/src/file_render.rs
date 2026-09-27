//! A vault file render: the text write-back puts on disk, and every stored
//! value that text does not hold.

use std::fmt;

use crate::entity_uri::EntityUri;

#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a render's losses are the only record that a value is missing from the file"]
pub struct Rendered {
    pub text: String,
    pub losses: Vec<RenderLoss>,
}

impl Rendered {
    pub fn faithful(text: String) -> Self {
        Self {
            text,
            losses: Vec::new(),
        }
    }
}

/// One stored value the file leaves out, or holds in a form that reads back
/// as something else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderLoss {
    pub block: EntityUri,
    pub detail: String,
}

impl fmt::Display for RenderLoss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "block {}: {}", self.block, self.detail)
    }
}
