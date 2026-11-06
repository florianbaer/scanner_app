//! Web front-end: an HTTP API plus a small browser UI on top of `ScannerService`.
//! This crate only translates HTTP ⇄ core calls; all scanning logic is in core.

mod dto;
mod error;
mod routes;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use scanner_core::service::{ScannerService, ServiceConfig};
use scanner_sane::SaneBackend;

#[derive(Parser)]
#[command(
    version,
    about = "Web UI for scanning: discover scanners, start scans, download PDFs"
)]
struct Args {
    /// Address to listen on. Use 0.0.0.0 to allow other devices on your network
    /// (there is no authentication).
    #[arg(long, env = "SCANNER_HOST", default_value = "127.0.0.1")]
    host: std::net::IpAddr,

    #[arg(short, long, env = "SCANNER_PORT", default_value_t = 8080)]
    port: u16,

    /// Directory where finished scans are stored.
    #[arg(short, long, env = "SCANNER_OUT", default_value = "scans")]
    out: PathBuf,

    /// Destinations config (TOML). Defaults to ./scanner.toml if it exists.
    #[arg(short, long, env = "SCANNER_CONFIG")]
    config: Option<PathBuf>,

    /// Minutes between retries of a failed upload.
    #[arg(long, env = "SCANNER_RETRY_MINUTES", default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    retry_minutes: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let backend = SaneBackend::new().context("initialising SANE")?;
    let destinations = scanner_destinations::load_configured(args.config.as_deref())
        .context("loading destinations")?;
    for d in &destinations {
        let info = d.info();
        eprintln!("Destination: {} ({}, {})", info.label, info.id, info.kind);
    }
    let mut config = ServiceConfig::new(args.out.clone());
    config.destinations = destinations;
    config.retry_interval = Duration::from_secs(args.retry_minutes * 60);
    let service = Arc::new(ScannerService::start(backend, config)?);
    let pending = service.pending_uploads().len();
    if pending > 0 {
        eprintln!("{pending} upload(s) pending from before; retrying now");
    }

    let addr = SocketAddr::new(args.host, args.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    eprintln!(
        "Scanner web UI on http://{addr} (scans in {})",
        args.out.display()
    );

    axum::serve(listener, routes::router(service))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
