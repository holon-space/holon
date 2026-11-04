//! Polling utilities for waiting on async conditions

use std::future::Future;
use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;

/// One look at a wait's target.
pub enum Probe<T> {
    Reached(T),
    /// The target is not reached; the value names where it stands now.
    NotYet(String),
}

/// A wait whose target was not reached before its hang deadline.
#[derive(Debug)]
pub struct WaitHung {
    pub target: String,
    pub waited: Duration,
    pub last_value: String,
}

impl std::fmt::Display for WaitHung {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "target {} not reached after {:?}; last value: {}",
            self.target, self.waited, self.last_value
        )
    }
}

impl std::error::Error for WaitHung {}

/// Probe `target` until it is reached. `deadline` only detects a hang: the
/// `Err` names the target and the last value the probe saw.
pub async fn wait_until<T, F, Fut>(
    target: &str,
    deadline: Duration,
    mut probe: F,
) -> Result<T, WaitHung>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Probe<T>>,
{
    let start = Instant::now();
    loop {
        let last_value = match probe().await {
            Probe::Reached(value) => return Ok(value),
            Probe::NotYet(last_value) => last_value,
        };
        if start.elapsed() >= deadline {
            return Err(WaitHung {
                target: target.to_string(),
                waited: start.elapsed(),
                last_value,
            });
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Wait until the file content satisfies `condition` (reads through the
/// `FileSystem` port — ADR 0011).
pub async fn wait_for_file_condition<F>(
    fs: &dyn holon_filesystem::FileSystem,
    file_path: &Path,
    condition: F,
    deadline: Duration,
) -> Result<String, WaitHung>
where
    F: Fn(&str) -> bool,
{
    wait_until(
        &format!("content of {} matches the condition", file_path.display()),
        deadline,
        || async {
            match fs.read_to_string(file_path).await {
                Ok(content) if condition(&content) => Probe::Reached(content),
                Ok(content) => Probe::NotYet(content),
                Err(e) => Probe::NotYet(format!("unreadable: {e}")),
            }
        },
    )
    .await
}

/// Wait until `BoundsRegistry` has committed bounds for the given block
/// entity. `GpuiUserDriver` fails loud when bounds aren't available —
/// call this after any transition that creates or re-parents a block,
/// before driving user input against it.
///
/// The `BoundsRegistry` double-buffers staged → committed per render
/// pass, so a just-added element stays invisible to the driver until the
/// next `begin_pass`. This polls the registry with a short interval and
/// returns the committed `ElementInfo` or an error on timeout.
pub async fn wait_for_element_bounds(
    geometry: &dyn GeometryProvider,
    entity_id: &str,
    timeout: Duration,
) -> Result<ElementInfo> {
    let el_id = format!("render-entity-{entity_id}");
    let poll_interval = Duration::from_millis(5);
    let deadline = Instant::now() + timeout;

    loop {
        if let Some(info) = geometry.element_info(&el_id) {
            return Ok(info);
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "wait_for_element_bounds: timed out after {timeout:?} waiting for bounds of \
                 entity {entity_id:?} (element id {el_id:?}) — element was never rendered, or \
                 BoundsRegistry never promoted staged → committed. Check that the element \
                 actually entered the render tree after the triggering mutation."
            ))
            .with_context(|| format!("entity {entity_id}"));
        }
        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use proptest::prelude::*;

    use super::*;

    /// A target the probe reaches on its `reached_at`-th look, or never.
    async fn wait_for_count(reached_at: Option<u32>, deadline: Duration) -> Result<u32, WaitHung> {
        let looks = Cell::new(0u32);
        wait_until("the counter", deadline, || {
            looks.set(looks.get() + 1);
            let look = looks.get();
            async move {
                match reached_at {
                    Some(at) if look >= at => Probe::Reached(look),
                    _ => Probe::NotYet(format!("look {look}")),
                }
            }
        })
        .await
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

        /// A reachable target returns its value on the first look that
        /// reaches it, however long that takes within the deadline.
        #[test]
        fn a_reached_target_returns_the_probe_value(at in 1u32..20) {
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
            let got = rt.block_on(wait_for_count(Some(at), Duration::from_secs(30)));
            prop_assert_eq!(got.map_err(|e| e.to_string()), Ok(at));
        }
    }

    /// A target never reached fails at the deadline, and the error names the
    /// target and the last value the probe saw.
    #[tokio::test]
    async fn a_hang_names_the_target_and_its_last_value() {
        let hung = wait_for_count(None, Duration::from_millis(50))
            .await
            .expect_err("the counter is never reached");
        let message = hung.to_string();
        assert!(
            message.starts_with("target the counter not reached after ")
                && message.ends_with(&format!("last value: {}", hung.last_value))
                && hung.last_value.starts_with("look "),
            "{message}"
        );
        assert!(hung.waited >= Duration::from_millis(50), "{message}");
    }
}
