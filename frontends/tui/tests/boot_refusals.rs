//! What `holon-tui` refuses to start, or to serve, at boot.
//!
//! @pbt kind harness
//! @pbt covers tui-boot-refusals — a missing vault root stops the boot with
//! an error naming it, on the terminal and in the log; `--mcp-enabled false`
//! boots with no MCP listener; a refused second instance leaves the running
//! instance's log intact

mod tui_process;

use tui_process::BOOTED;
use tui_process::Launch;

#[test]
fn a_missing_vault_root_refuses_to_start_and_names_it() {
    let parent = tempfile::tempdir().expect("parent dir");
    let missing = parent.path().join("mistyped-vault");
    let mut tui = Launch::new(18767).vault_root(missing.clone()).spawn(|_| {});
    let status = tui.wait_for_exit("a boot on a missing vault");

    let refusal = format!(
        "refusing to start: vault {} does not exist or is not a directory",
        missing.display()
    );
    let (log, terminal) = (tui.log_text(), tui.terminal_text());
    assert!(
        !status.success(),
        "a boot on a missing vault must exit non-zero; log:\n{log}"
    );
    // The terminal report is colored, wraps at its width and frames each
    // line with `│`.
    let compact = |text: &str| {
        let mut chars = text.chars();
        let mut kept = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\u{1b}' => {
                    chars.find(|c| c.is_ascii_alphabetic());
                }
                '│' => {}
                c if c.is_whitespace() => {}
                c => kept.push(c),
            }
        }
        kept
    };
    assert!(
        compact(&terminal).contains(&compact(&format!("× {refusal}"))),
        "the terminal must show `{refusal}` as the error itself; it shows:\n{terminal}"
    );
    assert!(
        log.contains(&refusal),
        "the log must record `{refusal}`; log:\n{log}"
    );
    assert!(
        !missing.exists(),
        "the refused boot created {}",
        missing.display()
    );
}

#[test]
fn mcp_disabled_serves_no_mcp() {
    let port = 18768;
    let mut tui = Launch::new(port)
        .arg("--mcp-enabled")
        .arg("false")
        .spawn(|_| {});
    tui.wait_for_log(BOOTED);
    // An enabled server binds shortly after the boot reports ready.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut listener = std::net::TcpStream::connect(("127.0.0.1", port));
    while listener.is_err() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
        listener = std::net::TcpStream::connect(("127.0.0.1", port));
    }
    tui.signal("TERM");
    let status = tui.wait_for_exit("SIGTERM");

    let text = tui.log_text();
    assert!(
        listener.is_err(),
        "`--mcp-enabled false` must bind no MCP listener, but 127.0.0.1:{port} accepted a \
         connection; log:\n{text}"
    );
    assert!(
        text.contains("MCP server disabled"),
        "the log must disclose that MCP is off; log:\n{text}"
    );
    assert!(
        status.success(),
        "holon-tui must quit cleanly, got {status}; log:\n{text}"
    );
}

#[test]
fn a_refused_second_instance_leaves_the_running_instances_log_intact() {
    let mut first = Launch::new(18769).spawn(|_| {});
    first.wait_for_log(BOOTED);
    let before = first.log_text();

    let mut second = Launch::new(18770)
        .home(first.home().to_path_buf())
        .vault_root(first.vault().to_path_buf())
        .spawn(|_| {});
    let status = second.wait_for_exit("a second instance on the first one's vault");

    let after = first.log_text();
    first.signal("TERM");
    first.wait_for_exit("SIGTERM");
    assert!(
        !status.success(),
        "a second instance on a locked vault must be refused; log:\n{after}"
    );
    assert!(
        after.starts_with(&before),
        "the refused instance must leave the running instance's log intact; \
         before:\n{before}\nafter:\n{after}"
    );
}
