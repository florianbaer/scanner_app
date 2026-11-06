//! CLI front-end: parses arguments, calls the core library, prints results.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use scanner_core::destination::{self, SharedDestination};
use scanner_core::output::{self, Format, OutputSpec};
use scanner_core::scanner::{Backend, ColorMode, FoundDevice, ScanSettings, Side, Source, Until};
use scanner_sane::{fmt_num, Constraint, SaneBackend, ValueType};

#[derive(Parser)]
#[command(
    version,
    about = "Scan wirelessly from an Epson FastFoto FF-680W (or any SANE scanner)"
)]
struct Cli {
    /// Destinations config (TOML). Defaults to ./scanner.toml if it exists.
    #[arg(short, long, global = true, env = "SCANNER_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find scanners on the network and over USB.
    List(DiscoverArgs),
    /// Show all options the scanner supports and their current values.
    Options(DeviceArgs),
    /// Scan everything in the feeder.
    Scan(ScanArgs),
    /// List configured destinations (Paperless, S3, …).
    Destinations,
}

#[derive(Args)]
struct DiscoverArgs {
    /// How long to listen for network scanners, in seconds.
    #[arg(long, default_value_t = 8)]
    timeout: u64,
}

#[derive(Args)]
struct DeviceArgs {
    /// SANE device name, e.g. epsonds:net:<ip>. Discovered automatically if omitted.
    #[arg(short, long, env = "SCANNER_DEVICE")]
    device: Option<String>,

    #[command(flatten)]
    discover: DiscoverArgs,
}

#[derive(Args)]
struct ScanArgs {
    #[command(flatten)]
    device: DeviceArgs,

    /// Resolution in DPI.
    #[arg(short, long, default_value_t = 300)]
    resolution: u32,

    #[arg(short, long, value_enum, default_value_t = ModeArg::Color)]
    mode: ModeArg,

    /// Scan both sides of each sheet.
    #[arg(long)]
    duplex: bool,

    /// Keep the full scan area instead of cropping to the photo/document.
    #[arg(long)]
    no_crop: bool,

    /// Don't straighten skewed sheets.
    #[arg(long)]
    no_deskew: bool,

    #[arg(short, long, value_enum, default_value_t = FormatArg::Pdf)]
    format: FormatArg,

    /// JPEG quality (also for images inside PDFs).
    #[arg(short, long, default_value_t = 90, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,

    /// Output directory.
    #[arg(short, long, default_value = "scans")]
    out: PathBuf,

    /// File name prefix; a timestamp is appended.
    #[arg(long, default_value = "scan")]
    name: String,

    /// Also send the result to this destination (see `destinations`). Repeatable.
    #[arg(long = "to", value_name = "ID")]
    to: Vec<String>,

    /// Delete the local copy once every --to destination accepted the upload.
    #[arg(long, requires = "to")]
    delete_after_upload: bool,

    /// Set a raw scanner option (see `options`), e.g. --set adf-skew=no. Repeatable.
    #[arg(long = "set", value_name = "NAME=VALUE", value_parser = parse_key_value)]
    extra: Vec<(String, String)>,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Color,
    Gray,
    Lineart,
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Pdf,
    Jpeg,
    Png,
}

fn parse_key_value(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .ok_or_else(|| format!("expected NAME=VALUE, got '{s}'"))
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let destinations = scanner_destinations::load_configured(cli.config.as_deref())
        .context("loading destinations")?;
    if let Command::Destinations = cli.command {
        return list_destinations(&destinations);
    }
    let backend = SaneBackend::new()?;
    match cli.command {
        Command::List(args) => list(&backend, &args),
        Command::Options(args) => options(&backend, &args),
        Command::Scan(args) => scan(&backend, &destinations, args),
        Command::Destinations => unreachable!("handled above"),
    }
}

fn list_destinations(destinations: &[SharedDestination]) -> Result<()> {
    if destinations.is_empty() {
        println!(
            "No destinations configured. Create {} (see scanner.example.toml) or pass --config.",
            scanner_destinations::DEFAULT_CONFIG
        );
    }
    for d in destinations {
        let info = d.info();
        println!("{}\t{}\t{}", info.id, info.kind, info.label);
    }
    Ok(())
}

fn list(backend: &SaneBackend, args: &DiscoverArgs) -> Result<()> {
    let devices = discover(backend, args, Until::Timeout);
    if devices.is_empty() {
        println!("No scanners found.");
    }
    for d in devices {
        println!("{}\t{}", d.name, d.label);
    }
    Ok(())
}

fn discover(backend: &SaneBackend, args: &DiscoverArgs, until: Until) -> Vec<FoundDevice> {
    eprintln!("Searching for scanners…");
    let result = backend.discover_detailed(Duration::from_secs(args.timeout), until);
    for (source, err) in &result.errors {
        eprintln!("warning: {source} discovery failed: {err:#}");
    }
    result.devices
}

fn resolve_device(backend: &SaneBackend, args: &DeviceArgs) -> Result<String> {
    if let Some(d) = &args.device {
        return Ok(d.clone());
    }
    let devices = discover(backend, &args.discover, Until::First);
    let Some(first) = devices.into_iter().next() else {
        bail!("no scanner found. Is it switched on and on the same network? You can also pass --device epsonds:net:<ip>");
    };
    eprintln!("Using {} ({})", first.label, first.name);
    Ok(first.name)
}

fn options(backend: &SaneBackend, args: &DeviceArgs) -> Result<()> {
    let name = resolve_device(backend, args)?;
    let scanner = backend.open_sane(&name)?;
    let dev = scanner.device();
    for opt in dev.options() {
        if opt.value_type == ValueType::Group || opt.name.is_empty() {
            println!("\n{}:", opt.title);
            continue;
        }
        let value = dev
            .get(&opt)
            .map(|v| v.to_string())
            .unwrap_or_else(|e| format!("<{e}>"));
        let allowed = match &opt.constraint {
            Constraint::None => String::new(),
            Constraint::Range { min, max, .. } => {
                format!("  [{}..{}]", fmt_num(*min), fmt_num(*max))
            }
            Constraint::List(v) => format!(
                "  [{}]",
                v.iter().map(|x| fmt_num(*x)).collect::<Vec<_>>().join("|")
            ),
            Constraint::Strings(v) => format!("  [{}]", v.join("|")),
        };
        let flags = if !opt.is_active() {
            "  (inactive)"
        } else if !opt.is_settable() {
            "  (read-only)"
        } else {
            ""
        };
        println!("  {:<20} = {value}{allowed}{flags}", opt.name);
    }
    Ok(())
}

fn scan(backend: &SaneBackend, destinations: &[SharedDestination], args: ScanArgs) -> Result<()> {
    // Validate destinations before feeding any paper.
    let targets = destination::select(destinations, &args.to).map_err(anyhow::Error::msg)?;
    let name = resolve_device(backend, &args.device)?;
    let mut scanner = backend
        .open(&name)
        .context("could not connect to the scanner")?;

    let settings = ScanSettings {
        mode: match args.mode {
            ModeArg::Color => ColorMode::Color,
            ModeArg::Gray => ColorMode::Gray,
            ModeArg::Lineart => ColorMode::Lineart,
        },
        source: if args.duplex {
            Source::FeederDuplex
        } else {
            Source::Feeder
        },
        dpi: args.resolution,
        auto_crop: !args.no_crop,
        deskew: !args.no_deskew,
        extra_options: args.extra,
    };
    let format = match args.format {
        FormatArg::Pdf => Format::Pdf,
        FormatArg::Jpeg => Format::Jpeg,
        FormatArg::Png => Format::Png,
    };
    let spec = OutputSpec::timestamped(args.out.clone(), &args.name, format, args.quality);

    let mut sink = output::create_sink(&spec)?;
    eprintln!("Scanning at {} dpi…", settings.dpi);
    let count = scanner.scan(&settings, &mut |page| {
        let side = match page.side {
            Side::Single => "",
            Side::Front => " (front)",
            Side::Back => " (back)",
        };
        eprintln!(
            "  sheet {}{side}: {}×{} px",
            page.sheet,
            page.image.width(),
            page.image.height()
        );
        sink.add(&page)
    });

    // Keep whatever was scanned even if the batch failed midway (e.g. a paper jam).
    let files = sink.finish()?;
    let count = count?;
    eprintln!("Scanned {count} page(s).");
    for f in &files {
        println!("{}", f.display());
    }

    if targets.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let report = destination::deliver_files(&targets, &args.out, &names, args.delete_after_upload);
    let mut failed = 0;
    for d in &report.deliveries {
        match &d.result {
            Ok(location) => eprintln!("  ✓ {} → {}: {location}", d.file, d.destination),
            Err(e) => {
                failed += 1;
                let to = if d.destination.is_empty() {
                    "local"
                } else {
                    &d.destination
                };
                eprintln!("  ✗ {} → {to}: {e}", d.file);
            }
        }
    }
    for f in &report.deleted {
        eprintln!("  removed local copy {f}");
    }
    if failed > 0 {
        bail!("{failed} upload(s) failed; local files were kept");
    }
    Ok(())
}
