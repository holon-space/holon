use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;

use crate::config::{self, HolonConfig, SessionConfig};
use crate::preferences::PrefKey;

/// Parse CLI args via clap, load layered config (Defaults → TOML → CLI/env),
/// and return everything needed to construct a `FrontendSession`.
///
/// This is the one-liner for CLI frontends:
/// ```rust,ignore
/// let (holon_config, session_config, config_dir, locked) =
///     cli::build_session(widgets)?;
/// let session = FrontendSession::new_from_config(
///     holon_config, session_config, config_dir, locked,
/// ).await?;
/// ```
pub fn build_session(
    available_widgets: HashSet<String>,
) -> Result<(HolonConfig, SessionConfig, PathBuf, HashSet<PrefKey>)> {
    use clap::Parser;

    let cli_parsed = HolonConfig::try_parse().map_err(|e| anyhow::anyhow!("{e}"))?;

    let config_dir = config::resolve_config_dir(cli_parsed.config_dir.as_deref());

    let (traced, locked) = config::load_config(&config_dir, cli_parsed)?;
    let holon_config = traced.into_inner();

    let ui_info = holon_api::UiInfo {
        available_widgets,
        screen_size: None,
    };
    let session_config = SessionConfig::new(ui_info);

    eprintln!(
        "Config dir: {}, db: {}, orgmode: {:?}, loro: {}",
        config_dir.display(),
        holon_config.resolve_db_path(&config_dir).display(),
        holon_config.orgmode.root_directory,
        holon_config.loro_enabled(),
    );

    Ok((holon_config, session_config, config_dir, locked))
}
