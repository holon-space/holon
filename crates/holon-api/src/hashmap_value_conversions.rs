//! Entity-macro coverage for the HashMap <-> Value conversions.
//!
//! The conversions themselves live in `holon-pattern` (they are impls on
//! `Value` with no local type); these tests exercise them through the `Entity`
//! derive, which only exists here.

#[cfg(test)]
mod tests;
