//! `holon-connection` — ask this build which connections it would admit.
//!
//! The enable script needs to know whether a name exists before it writes a
//! state file for it, because a state file for a name nothing provides is read
//! by nothing: the script would report success and do nothing. It used to
//! answer that question by parsing `bundled_sidecars.rs` and listing `*.yaml`
//! in the directory — a second implementation of the admission rules, in bash.
//!
//! It drifted, which is the whole reason this binary exists: the loader refuses
//! a SYMLINKED sidecar (a link lets a file outside the directory decide what a
//! connection calls and which secrets it may name) and the script's glob
//! happily switched one on, so the script and the app disagreed about which
//! connections there are. Every such rule added on either side would have to be
//! added twice.
//!
//! This prints what [`ConnectionRoster::scan_loadable`] — the loader's own
//! answer — says, so there is one implementation and the script reads it.
//!
//! `scan_loadable`, not `scan`: the scan settles the rules that read a file's
//! name and its neighbours, and the rest are settled when the content is
//! loaded. Asking only the scan is how the script came to switch on a
//! connection whose INTEGER identity column the app then refused.

use holon_mcp_client::ConnectionRoster;

const USAGE: &str = "\
holon-connection — what this build would admit from an integrations directory

USAGE:
    holon-connection list <DIR>   one admitted connection name per line

Prints the connections this build would run if they were switched on: the ones
it bundles, plus the ones a usable file in DIR introduces. Files DIR holds that
name no connection — an unusable name, a symlink, a duplicate, or content this
build refuses to load — are reported on stderr with the reason, and are NOT
listed.

Exit 0 with a list, 1 on an unreadable directory, 2 on a usage error.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match refs.as_slice() {
        ["list", dir] => list(std::path::Path::new(dir)),
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}

fn list(dir: &std::path::Path) {
    let roster = match ConnectionRoster::scan_loadable(dir) {
        Ok(roster) => roster,
        Err(e) => {
            eprintln!("holon-connection: cannot read '{}': {e:#}", dir.display());
            std::process::exit(1);
        }
    };
    for rejected in roster.rejected() {
        eprintln!(
            "holon-connection: ignoring '{}' — {}",
            rejected.path.display(),
            rejected.reason
        );
    }
    for name in roster.names() {
        println!("{name}");
    }
}
