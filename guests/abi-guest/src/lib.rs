//! Guest half of the five-function plugin ABI.
//!
//! A guest crate implements one function
//! `fn(input: &[u8], ctx: &[u8]) -> Result<String, String>` and hands it to
//! [`holon_plugin`], which emits the five `#[no_mangle]` exports the host
//! looks up. The guest is a pure function: it never touches WASI.

use std::cell::Cell;
use std::cell::RefCell;

thread_local! {
    static LAST_ERROR: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static LIVE_BYTES: Cell<u64> = const { Cell::new(0) };
}

#[inline]
pub fn pack(ptr: u32, len: u32) -> u64 {
    ((ptr as u64) << 32) | (len as u64)
}

/// Hand a heap buffer of `len` bytes to the host. The host owns it until it
/// calls `holon_dealloc` with the same `(ptr, len)`.
pub fn alloc(len: u32) -> u32 {
    if len == 0 {
        return 1; // aligned, never dereferenced
    }
    let ptr = unsafe { std::alloc::alloc(layout(len)) };
    assert!(!ptr.is_null(), "guest out of memory for {len} bytes");
    lent(len);
    ptr as u32
}

/// # Safety
/// `ptr`/`len` must be a span previously returned by [`alloc`] or by
/// [`leak_output`], not yet freed.
pub unsafe fn dealloc(ptr: u32, len: u32) {
    if len == 0 {
        return;
    }
    returned(len);
    std::alloc::dealloc(ptr as *mut u8, layout(len));
}

/// Bytes this guest handed to the host and has not got back. Every call must
/// leave it where it started: growth across calls is the host forgetting a
/// `holon_dealloc` on some exit path, which no other observable would show.
pub fn live_bytes() -> u64 {
    LIVE_BYTES.with(Cell::get)
}

fn lent(len: u32) {
    LIVE_BYTES.with(|live| live.set(live.get() + len as u64));
}

fn returned(len: u32) {
    LIVE_BYTES.with(|live| {
        let held = live.get();
        assert!(
            held >= len as u64,
            "host returned {len} bytes while the guest lent only {held}"
        );
        live.set(held - len as u64);
    });
}

/// Every buffer crossing the boundary uses this exact layout in both
/// directions, so `dealloc(ptr, len)` is sound whichever side allocated it.
fn layout(len: u32) -> std::alloc::Layout {
    std::alloc::Layout::from_size_align(len as usize, 1).expect("byte layout is always valid")
}

/// # Safety
/// `ptr`/`len` must denote a live buffer inside this module's linear memory.
pub unsafe fn borrow<'a>(ptr: u32, len: u32) -> &'a [u8] {
    core::slice::from_raw_parts(ptr as *const u8, len as usize)
}

pub fn leak_output(s: String) -> u64 {
    let bytes = s.into_bytes().into_boxed_slice();
    let len = bytes.len() as u32;
    let ptr = Box::into_raw(bytes) as *mut u8 as u32;
    lent(len);
    pack(ptr, len)
}

pub fn set_last_error(msg: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = msg.into_bytes());
}

pub fn last_error() -> u64 {
    LAST_ERROR.with(|e| {
        let b = e.borrow();
        pack(b.as_ptr() as u32, b.len() as u32)
    })
}

/// Emit the five ABI exports around `$body`, a path to a
/// `fn(&[u8], &[u8]) -> Result<String, String>`.
#[macro_export]
macro_rules! holon_plugin {
    ($body:path) => {
        #[no_mangle]
        pub extern "C" fn holon_alloc(len: u32) -> u32 {
            $crate::alloc(len)
        }

        #[no_mangle]
        pub unsafe extern "C" fn holon_dealloc(ptr: u32, len: u32) {
            $crate::dealloc(ptr, len)
        }

        #[no_mangle]
        pub unsafe extern "C" fn holon_parse(
            in_ptr: u32,
            in_len: u32,
            ctx_ptr: u32,
            ctx_len: u32,
        ) -> u64 {
            let input = $crate::borrow(in_ptr, in_len);
            let ctx = $crate::borrow(ctx_ptr, ctx_len);
            match $body(input, ctx) {
                Ok(out) => $crate::leak_output(out),
                Err(msg) => {
                    $crate::set_last_error(msg);
                    0
                }
            }
        }

        #[no_mangle]
        pub extern "C" fn holon_last_error() -> u64 {
            $crate::last_error()
        }

        #[no_mangle]
        pub extern "C" fn holon_live_bytes() -> u64 {
            $crate::live_bytes()
        }
    };
}
