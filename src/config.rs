use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result, ensure};

/// Startup configuration. Deliberately not Debug: it contains a secret.
pub struct Config {
    /// Listen address; loopback by default.
    pub bind: SocketAddr,
    /// Service-to-service bearer token; never supplied by ordinary QQ players.
    pub api_token: String,
    /// Directory containing the persistent SQLite database.
    pub data_dir: PathBuf,
    /// Directory for daily JSON log files.
    pub log_dir: PathBuf,
    /// Optional verified asset manifest; sibling assets.zip is loaded once.
    pub asset_manifest: Option<PathBuf>,
    /// Optional font archive manifest; includes the original OFL license.
    pub font_manifest: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let api_token =
            env::var("FARM_API_TOKEN").context("FARM_API_TOKEN must be set to a random token")?;
        ensure!(
            api_token.len() >= 32
                && api_token.len() <= 256
                && api_token.bytes().all(|b| b.is_ascii_graphic()),
            "FARM_API_TOKEN must contain 32..=256 visible ASCII characters"
        );
        Ok(Self {
            bind: env::var("FARM_BIND")
                .unwrap_or_else(|_| "127.0.0.1:8080".into())
                .parse()
                .context("invalid FARM_BIND address")?,
            api_token,
            asset_manifest: env::var_os("FARM_ASSET_MANIFEST").map(PathBuf::from),
            font_manifest: env::var_os("FARM_FONT_MANIFEST").map(PathBuf::from),
            data_dir: env::var_os("FARM_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("data")),
            log_dir: env::var_os("FARM_LOG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("logs")),
        })
    }
}
