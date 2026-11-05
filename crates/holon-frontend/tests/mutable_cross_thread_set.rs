//! A `Mutable::set` that lands inside the subscriber's `poll_change`, after its
//! changed check and before it stores the waker, must still reach the
//! subscriber.
//!
//! The waker's clone hook calls `set`: `poll_change` clones the waker while it
//! stores it, so the set lands in that window on every run.

use std::cell::RefCell;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::task::RawWaker;
use std::task::RawWakerVTable;
use std::task::Waker;

use futures_signals::signal::Mutable;
use futures_signals::signal::Signal;

thread_local! {
    static SET_ON_CLONE: RefCell<Option<Mutable<u32>>> = const { RefCell::new(None) };
}

unsafe fn clone(_: *const ()) -> RawWaker {
    if let Some(mutable) = SET_ON_CLONE.with(|cell| cell.borrow_mut().take()) {
        mutable.set(1);
    }
    RawWaker::new(std::ptr::null(), &VTABLE)
}

unsafe fn noop(_: *const ()) {}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);

#[test]
fn a_set_during_the_subscribers_poll_is_not_lost() {
    let mutable = Mutable::new(0);
    let mut signal = mutable.signal();
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);

    assert_eq!(
        Pin::new(&mut signal).poll_change(&mut cx),
        Poll::Ready(Some(0))
    );
    SET_ON_CLONE.with(|cell| *cell.borrow_mut() = Some(mutable.clone()));
    assert_eq!(
        Pin::new(&mut signal).poll_change(&mut cx),
        Poll::Ready(Some(1)),
        "the set inside poll_change was lost: the subscriber stays Pending"
    );
}
