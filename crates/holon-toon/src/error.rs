//! Fail-loud error type for the TOON projection layer.
//!
//! Per the repo's "Parse, Don't Validate" discipline, every fallible boundary
//! (scalar decode, row split, props decode, tree reconstruction) returns
//! [`Result`] with an enriched message — no `.ok()`, no silent defaults.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToonError {
    #[error("empty TOON document (expected a `blocks[N]{{...}}:` header)")]
    EmptyDocument,

    #[error("expected table header `blocks[N]{{id,depth,state,props,body,title}}:`, got {got:?}")]
    BadHeader { got: String },

    #[error(
        "declared row count {declared} does not match the {actual} data rows that followed the \
         header"
    )]
    RowCountMismatch { declared: usize, actual: usize },

    #[error("row {row}: expected {expected} comma-separated cells, found {found}: {line:?}")]
    CellCountMismatch {
        row: usize,
        expected: usize,
        found: usize,
        line: String,
    },

    #[error("row {row}: `depth` cell {cell:?} is not a non-negative integer")]
    BadDepth { row: usize, cell: String },

    #[error(
        "row {row}: depth {depth} jumps by more than one from the previous depth {prev} — a \
         well-formed pre-order forest only ever descends one level at a time"
    )]
    DepthJump { row: usize, depth: u16, prev: u16 },

    #[error("row {row}: first block has depth {depth}, but a forest root must have depth 0")]
    NonRootStart { row: usize, depth: u16 },

    #[error("row {row}: unterminated quoted string in {context}: {token:?}")]
    UnterminatedQuote {
        row: usize,
        context: String,
        token: String,
    },

    #[error("row {row}: invalid escape sequence {escape:?} in {context}")]
    BadEscape {
        row: usize,
        context: String,
        escape: String,
    },

    #[error("row {row}: `\\u` escape needs exactly four hex digits, got {got:?}")]
    BadUnicodeEscape { row: usize, got: String },

    #[error("row {row}: malformed props entry {entry:?} (expected `key=value`)")]
    BadPropsEntry { row: usize, entry: String },

    #[error("row {row}: reserved props key {key:?} has invalid value {value:?}: {reason}")]
    BadReservedProp {
        row: usize,
        key: String,
        value: String,
        reason: String,
    },

    #[error("row {row}: block id must be non-empty and contain no whitespace, got {id:?}")]
    BadBlockId { row: usize, id: String },

    #[error("row {row}: task state keyword must be a single whitespace-free word, got {state:?}")]
    BadState { row: usize, state: String },

    // --- generic tabular codec (`table.rs`) ---
    #[error(
        "table name {name:?} is not representable as a TOON array key (no whitespace, and none of \
         the structural chars `[ ] {{ }} , : \" \\` or control chars)"
    )]
    BadTableName { name: String },

    #[error(
        "non-finite float {value} cannot be encoded as a TOON scalar (NaN/∞ have no numeric \
         literal); quote it as a string upstream if intended"
    )]
    NonFiniteFloat { value: String },

    #[error("expected a generic table header `name[N]{{col,col,...}}:`, got {got:?}")]
    BadTableHeader { got: String },

    #[error("row {row}: bare numeric token {token:?} is not a representable i64/finite-f64 number")]
    BadNumber { row: usize, token: String },
}

pub type Result<T> = std::result::Result<T, ToonError>;
