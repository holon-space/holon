//! A shipped build must unwind, or no catch boundary runs: the panic ends the
//! process before `spawn_disclosing_panic` can disclose it. Run under the
//! shipped profile with `cargo test --release -p holon-api --test
//! release_build_unwinds`.

use std::process::Command;

const MESSAGE: &str = "panic-unwind probe: the task panicked";

#[test]
fn a_panicking_task_is_disclosed_and_the_process_lives() {
    let output = Command::new(env!("CARGO_BIN_EXE_panic-unwind-probe"))
        .arg(MESSAGE)
        .output()
        .expect("spawn the probe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the probe died of its task's panic ({}): this profile aborts on panic\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains(&format!("disclosed: {MESSAGE}")),
        "the panic was not disclosed as a TaskPanicked condition\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("alive after the panic"),
        "stdout:\n{stdout}"
    );
}
