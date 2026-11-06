//! Web front-end: an HTTP API plus a small browser UI on top of `ScannerService`.
//! This crate only translates HTTP ⇄ core calls; all scanning logic is in core.

mod dto;
mod error;
mod routes;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use scanner_core::service::ScannerService;
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let backend = SaneBackend::new().context("initialising SANE")?;
    let service = Arc::new(ScannerService::start(backend, args.out.clone())?);

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
