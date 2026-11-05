//! Waits for a row subscription to deliver a prop.

use std::time::Duration;

use futures_signals::signal::SignalExt;
use holon_api::Value;
use holon_frontend::ReactiveViewModel;

/// Only detects a hang; a delivering subscription lands far inside it.
const HANG: Duration = Duration::from_secs(5);

/// Resolves once `vm`'s `key` prop equals `expected`; panics with `context`
/// and the last props after `HANG`.
pub async fn prop_reaches(vm: &ReactiveViewModel, key: &str, expected: Value, context: &str) {
    let reached = vm
        .props
        .signal_ref(|props| props.get(key) == Some(&expected))
        .wait_for(true);
    if tokio::time::timeout(HANG, reached).await.is_err() {
        panic!(
            "{context}: prop `{key}` never became {expected:?} within {HANG:?}; last props: {:?}",
            vm.props.get_cloned()
        );
    }
}
