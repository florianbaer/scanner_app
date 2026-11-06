//! Destinations for finished scans, built from a TOML config file.
//! Implements `scanner_core::destination::Destination`; front-ends call
//! [`load`] and hand the result to the scan service.

pub mod config;
mod multipart;
mod paperless;
mod s3;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use scanner_core::destination::SharedDestination;

pub use config::Config;

/// Config file used when none is given explicitly.
pub const DEFAULT_CONFIG: &str = "scanner.toml";

/// Loads destinations from `explicit` (must exist), or from [`DEFAULT_CONFIG`]
/// if that file exists, or returns none.
pub fn load_configured(explicit: Option<&Path>) -> Result<Vec<SharedDestination>> {
    match explicit {
        Some(path) => load(path),
        None if Path::new(DEFAULT_CONFIG).is_file() => load(Path::new(DEFAULT_CONFIG)),
        None => Ok(Vec::new()),
    }
}

/// Loads and builds all destinations from a config file.
pub fn load(path: &Path) -> Result<Vec<SharedDestination>> {
    build(&Config::from_file(path)?)
}

/// Builds destinations from a parsed config.
pub fn build(config: &Config) -> Result<Vec<SharedDestination>> {
    config
        .destinations
        .iter()
        .map(|d| {
            let label = d.label.clone().unwrap_or_else(|| d.id.clone());
            let dest: SharedDestination = match &d.kind {
                config::Kind::Paperless(c) => {
                    Arc::new(paperless::Paperless::new(d.id.clone(), label, c))
                }
                config::Kind::S3(c) => Arc::new(
                    s3::S3::new(d.id.clone(), label, c)
                        .with_context(|| format!("destination '{}'", d.id))?,
                ),
            };
            Ok(dest)
        })
        .collect()
}
