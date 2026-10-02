//! Counts the projected docs' root deliveries by whether the delivering thread
//! held that doc's write guard. A commit number minted in the callback is only
//! ordered before the writer's return when the delivery is guarded; an
//! unguarded one is a commit outside the guard or an emission loro deferred to
//! another thread.
//!
//! With `HOLON_LORO_EMIT_PROBE_LOG=<path>` every delivery also appends one
//! line (`guarded|unguarded <trigger> <origin>`), so a run that spans many
//! test processes can be totalled afterwards.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;

static COUNTS: Mutex<Option<HashMap<(String, bool), usize>>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmitCounts {
    pub guarded: usize,
    pub unguarded: usize,
}

/// This process's deliveries so far whose origin contains `origin`.
pub fn counts_for(origin: &str) -> EmitCounts {
    let counts = COUNTS.lock().unwrap();
    let mut out = EmitCounts::default();
    for ((o, guarded), n) in counts.iter().flatten() {
        if o.contains(origin) {
            if *guarded {
                out.guarded += n;
            } else {
                out.unguarded += n;
            }
        }
    }
    out
}

pub(crate) fn record(guarded: bool, event: &loro::event::DiffEvent<'_>) {
    *COUNTS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .entry((event.origin.to_string(), guarded))
        .or_default() += 1;
    if let Some(path) = std::env::var_os("HOLON_LORO_EMIT_PROBE_LOG") {
        let label = if guarded { "guarded" } else { "unguarded" };
        let line = format!("{label} {:?} {:?}\n", event.triggered_by, event.origin);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .unwrap_or_else(|e| panic!("emit probe: appending to {path:?}: {e}"));
    }
}
