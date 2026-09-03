//! The five-function core-wasm ABI shared by the host and every guest.
//!
//! Values crossing the boundary are `(ptr, len)` pairs into the guest's linear
//! memory, packed into one `u64` as `(ptr as u64) << 32 | len as u64` so a
//! core-wasm function can return them without multi-value support.
//!
//! Not the component model: no runtime gives components on all three targets
//! this host must reach.

pub const EXPORT_MEMORY: &str = "memory";
pub const EXPORT_ALLOC: &str = "holon_alloc";
pub const EXPORT_DEALLOC: &str = "holon_dealloc";
pub const EXPORT_PARSE: &str = "holon_parse";
pub const EXPORT_LAST_ERROR: &str = "holon_last_error";
pub const EXPORT_LIVE_BYTES: &str = "holon_live_bytes";

#[inline]
pub fn pack(ptr: u32, len: u32) -> u64 {
    ((ptr as u64) << 32) | (len as u64)
}

#[inline]
pub fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, (v & 0xffff_ffff) as u32)
}
