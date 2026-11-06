//! CLI front-end: parses arguments, calls the core library, prints results.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use scanner_app::discovery::{self, MdnsEpsonSource, SaneSource, Until};
use scanner_app::output::{self, Format, OutputSpec};
use scanner_app::sane::{fmt_num, Constraint, Sane, SaneScanner, ValueType};
use scanner_app::scanner::{ColorMode, ScanError, ScanSettings, Scanner, Side, Source};

#[derive(Parser)]
#[command(
    version,
    about = "Scan wirelessly from an Epson FastFoto FF-680W (or any SANE scanner)"
)]
struct Cli {
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
        if e.downcast_ref::<ScanError>() == Some(&ScanError::FeederEmpty) {
            eprintln!("error: no documents in the feeder. Load photos or pages and try again.");
        } else {
            eprintln!("error: {e:#}");
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let sane = Sane::init()?;
    match cli.command {
        Command::List(args) => list(&sane, &args),
        Command::Options(args) => options(&sane, &args),
        Command::Scan(args) => scan(&sane, args),
    }
}

fn list(sane: &Sane, args: &DiscoverArgs) -> Result<()> {
    let devices = discover(sane, args, Until::Timeout)?;
    if devices.is_empty() {
        println!("No scanners found.");
    }
    for d in devices {
        println!("{}\t{}", d.name, d.label);
    }
    Ok(())
}

fn discover(sane: &Sane, args: &DiscoverArgs, until: Until) -> Result<Vec<discovery::FoundDevice>> {
    eprintln!("Searching for scanners…");
    let sane_src = SaneSource(sane.clone());
    let result = discovery::discover(
        &[&MdnsEpsonSource, &sane_src],
        Duration::from_secs(args.timeout),
        until,
    );
    for (source, err) in &result.errors {
        eprintln!("warning: {source} discovery failed: {err:#}");
    }
    Ok(result.devices)
}

fn resolve_device(sane: &Sane, args: &DeviceArgs) -> Result<String> {
    if let Some(d) = &args.device {
        return Ok(d.clone());
    }
    let devices = discover(sane, &args.discover, Until::First)?;
    let Some(first) = devices.into_iter().next() else {
        bail!("no scanner found. Is it switched on and on the same network? You can also pass --device epsonds:net:<ip>");
    };
    eprintln!("Using {} ({})", first.label, first.name);
    Ok(first.name)
}

fn options(sane: &Sane, args: &DeviceArgs) -> Result<()> {
    let name = resolve_device(sane, args)?;
    let scanner = SaneScanner::open(sane, &name)?;
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
                v.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join("|")
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

fn scan(sane: &Sane, args: ScanArgs) -> Result<()> {
    let name = resolve_device(sane, &args.device)?;
    let mut scanner = SaneScanner::open(sane, &name).context("could not connect to the scanner")?;

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
    let spec = OutputSpec {
        dir: args.out,
        stem: format!(
            "{}_{}",
            args.name,
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        ),
        format: match args.format {
            FormatArg::Pdf => Format::Pdf,
            FormatArg::Jpeg => Format::Jpeg,
            FormatArg::Png => Format::Png,
        },
        jpeg_quality: args.quality,
    };

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
    for f in files {
        println!("{}", f.display());
    }
    Ok(())
}
