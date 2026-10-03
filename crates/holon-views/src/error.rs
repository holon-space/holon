/// More rounds than any real outline needs; the bound SQL's ancestry walk uses
/// too. A recursion still deriving rows past it fails loud.
pub const MAX_DEPTH: u64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("a recursion still derived rows after {MAX_DEPTH} rounds")]
    DepthBound,
}
