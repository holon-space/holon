use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;

use crate::FrontendConfig;

pub struct CliConfig {
    pub db_path: Option<PathBuf>,
    pub orgmode_root: Option<PathBuf>,
    pub loro_enabled: bool,
}

impl CliConfig {
    /// Build a CliConfig from environment variables only (no CLI arg parsing).
    /// Useful for frontends like Dioxus that don't use CLI args.
    pub fn from_env() -> Self {
        Self {
            db_path: std::env::var("HOLON_DB_PATH").ok().map(PathBuf::from),
            orgmode_root: std::env::var("HOLON_ORGMODE_ROOT").ok().map(PathBuf::from),
            loro_enabled: std::env::var("HOLON_LORO_ENABLED")
                .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
                .unwrap_or(false),
        }
    }

    pub fn log_summary(&self, frontend_name: &str) {
        eprintln!(
            "{frontend_name} frontend: db={}, orgmode={:?}, loro={}",
            self.db_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or("(temp)".into()),
            self.orgmode_root,
            self.loro_enabled
        );
    }
}

pub fn parse_args(binary_name: &str) -> Result<CliConfig> {
    let mut args = std::env::args().skip(1);
    let mut db_path: Option<PathBuf> = std::env::var("HOLON_DB_PATH").ok().map(PathBuf::from);
    let mut orgmode_root: Option<PathBuf> =
        std::env::var("HOLON_ORGMODE_ROOT").ok().map(PathBuf::from);
    let mut loro_enabled = std::env::var("HOLON_LORO_ENABLED")
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--orgmode-root" | "--orgmode-dir" => {
                let path_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--orgmode-root requires a path argument"))?;
                orgmode_root = Some(PathBuf::from(path_str));
            }
            "--loro" => {
                loro_enabled = true;
            }
            "--help" | "-h" => {
                eprintln!("Usage: {binary_name} [OPTIONS] [DATABASE_PATH]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --orgmode-root PATH  OrgMode root directory");
                eprintln!("  --loro               Enable Loro CRDT layer");
                eprintln!("  --help, -h           Show this help message");
                std::process::exit(0);
            }
            _ => {
                if !arg.starts_with("--") {
                    db_path = Some(PathBuf::from(arg));
                }
            }
        }
    }

    Ok(CliConfig {
        db_path,
        orgmode_root,
        loro_enabled,
    })
}

pub fn build_frontend_config(
    cli: &CliConfig,
    available_widgets: HashSet<String>,
) -> FrontendConfig {
    let ui_info = holon_api::UiInfo {
        available_widgets,
        screen_size: None,
    };
    let mut config = FrontendConfig::new(ui_info);

    if let Some(ref db) = cli.db_path {
        config = config.with_db_path(db.clone());
    }
    if let Some(ref org) = cli.orgmode_root {
        config = config.with_orgmode(org.clone());
    }
    if cli.loro_enabled {
        config = config.with_loro();
    }

    config
}
