//! `holon-secret` — put an integration's `${VAR}` secret into the OS keychain.
//!
//! The headless path. The primary one is Settings → Integrations, which writes
//! the same entries; this exists for a scripted or agent-driven setup, and for
//! a machine with no GUI session.
//!
//! The VALUE is read from stdin and never taken as an argument: an argument is
//! visible in the process table to every process on the machine, which is how
//! a token leaks into a transcript without anyone printing it.

use std::io::Read;
use std::io::Write;

use anyhow::Context;
use anyhow::Result;
use holon_secrets::INTEGRATION_SECRET_SERVICE;
use holon_secrets::platform_keychain;
use holon_secrets::secret_account;

const USAGE: &str = "\
holon-secret — store the secrets an integration sidecar references by name

USAGE:
    holon-secret set <VAR>      read the value from STDIN and store it
    holon-secret unset <VAR>    remove it
    holon-secret list <VAR>...  say which of these names have a stored value

A sidecar never holds a secret; it holds a reference like ${GITHUB_TOKEN}.
This command fills that reference in. The value is read from STDIN so it never
appears in the process table:

    printf %s \"$TOKEN\" | holon-secret set GITHUB_TOKEN

`list` takes the names to check because a keychain cannot be enumerated by
service alone on every platform. It prints names and presence, never values.
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match refs.as_slice() {
        ["set", var] => set(var),
        ["unset", var] => unset(var),
        ["list", vars @ ..] if !vars.is_empty() => list(vars),
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}

/// Reject a name a sidecar could not reference anyway, so a typo fails here
/// rather than becoming an entry nothing ever reads.
fn check(var: &str) -> Result<()> {
    anyhow::ensure!(!var.trim().is_empty(), "a variable name must not be empty");
    anyhow::ensure!(
        var.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.'),
        "'{var}' is not a usable variable name — use letters, digits, '_' and '.'"
    );
    Ok(())
}

fn set(var: &str) -> Result<()> {
    check(var)?;
    let mut value = Vec::new();
    std::io::stdin()
        .read_to_end(&mut value)
        .context("failed to read the secret from stdin")?;
    // A trailing newline is what `echo` adds and almost never part of the
    // secret; stripping exactly one is the difference between a token that
    // authenticates and one that mysteriously does not.
    if value.last() == Some(&b'\n') {
        value.pop();
    }
    if value.last() == Some(&b'\r') {
        value.pop();
    }
    anyhow::ensure!(
        !value.is_empty(),
        "nothing arrived on stdin — pipe the value in, e.g. `printf %s \"$TOKEN\" | holon-secret \
         set {var}`"
    );

    let account = secret_account(var);
    platform_keychain(INTEGRATION_SECRET_SERVICE)
        .store(&account, &value)
        .with_context(|| {
            format!(
                "failed to store '{var}' in the keychain (service {INTEGRATION_SECRET_SERVICE})"
            )
        })?;
    // Length only. The value never reaches a stream this process writes.
    let mut err = std::io::stderr();
    writeln!(
        err,
        "stored '{var}' ({} bytes) as {INTEGRATION_SECRET_SERVICE}/{account}",
        value.len()
    )?;
    writeln!(err, "Restart Holon to pick it up.")?;
    Ok(())
}

fn unset(var: &str) -> Result<()> {
    check(var)?;
    let account = secret_account(var);
    platform_keychain(INTEGRATION_SECRET_SERVICE)
        .delete(&account)
        .with_context(|| format!("failed to remove '{var}' from the keychain"))?;
    eprintln!("removed '{var}' ({INTEGRATION_SECRET_SERVICE}/{account})");
    Ok(())
}

fn list(vars: &[&str]) -> Result<()> {
    let store = platform_keychain(INTEGRATION_SECRET_SERVICE);
    for var in vars {
        check(var)?;
        let account = secret_account(var);
        let present = store
            .load(&account)
            .with_context(|| format!("failed to read '{var}' from the keychain"))?
            .is_some();
        println!("{var}\t{}", if present { "stored" } else { "absent" });
    }
    Ok(())
}
