//! Domain model for scanning, independent of any backend or front-end.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use image::DynamicImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Color,
    Gray,
    Lineart,
}

impl ColorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ColorMode::Color => "color",
            ColorMode::Gray => "gray",
            ColorMode::Lineart => "lineart",
        }
    }
}

impl FromStr for ColorMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "color" | "colour" => Ok(ColorMode::Color),
            "gray" | "grey" => Ok(ColorMode::Gray),
            "lineart" => Ok(ColorMode::Lineart),
            _ => Err(format!("unknown color mode '{s}'")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Document feeder, front side only.
    Feeder,
    /// Document feeder, both sides.
    FeederDuplex,
}

#[derive(Debug, Clone)]
pub struct ScanSettings {
    pub mode: ColorMode,
    pub source: Source,
    pub dpi: u32,
    pub auto_crop: bool,
    pub deskew: bool,
    /// Backend-specific options passed through verbatim (name, value).
    pub extra_options: Vec<(String, String)>,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            mode: ColorMode::Color,
            source: Source::Feeder,
            dpi: 300,
            auto_crop: true,
            deskew: true,
            extra_options: Vec::new(),
        }
    }
}

impl ScanSettings {
    /// Checks the settings against what a scanner supports. Empty capability
    /// lists mean "unknown" and are not enforced.
    pub fn validate(&self, caps: &Capabilities) -> Result<(), String> {
        if !caps.resolutions.is_empty() && !caps.resolutions.contains(&self.dpi) {
            return Err(format!(
                "{} dpi is not supported; choose one of {:?}",
                self.dpi, caps.resolutions
            ));
        }
        if !caps.modes.is_empty() && !caps.modes.contains(&self.mode) {
            return Err(format!("{} mode is not supported", self.mode.as_str()));
        }
        if self.source == Source::FeederDuplex && !caps.duplex {
            return Err("this scanner cannot scan both sides".into());
        }
        Ok(())
    }
}

/// What a particular scanner supports, so front-ends only offer valid settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capabilities {
    pub resolutions: Vec<u32>,
    pub modes: Vec<ColorMode>,
    pub duplex: bool,
    pub auto_crop: bool,
    pub deskew: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Single-sided scan.
    Single,
    Front,
    Back,
}

/// One scanned side of one sheet.
pub struct Page {
    /// 1-based sheet number within the batch.
    pub sheet: usize,
    pub side: Side,
    pub dpi: u32,
    pub image: DynamicImage,
}

/// A scanner found on the network or a local bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundDevice {
    /// Device name to open with the backend, e.g. `epsonds:net:<ip>`.
    pub name: String,
    /// Human-readable label, e.g. `EPSON FF-680W`.
    pub label: String,
}

/// When a discovery may stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Until {
    /// Return as soon as any device is found (or the timeout expires).
    First,
    /// Listen for the whole timeout to find every device.
    Timeout,
}

/// Expected conditions a front-end may want to handle specially.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanError {
    FeederEmpty,
    Cancelled,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::FeederEmpty => {
                write!(
                    f,
                    "no documents in the feeder. Load photos or pages and try again"
                )
            }
            ScanError::Cancelled => write!(f, "scan cancelled"),
        }
    }
}

impl std::error::Error for ScanError {}

/// A device that can scan a batch of sheets from its feeder.
pub trait Scanner: Send {
    fn capabilities(&self) -> Capabilities;

    /// Scans until the feeder is empty, handing each page to `on_page` as it arrives.
    /// If `on_page` fails, scanning stops and that error is returned.
    /// Returns the number of pages scanned.
    fn scan(
        &mut self,
        settings: &ScanSettings,
        on_page: &mut dyn FnMut(Page) -> anyhow::Result<()>,
    ) -> anyhow::Result<usize>;
}

/// Entry point to a scanning technology: finds devices and opens them.
pub trait Backend: Send + 'static {
    fn discover(&self, timeout: Duration, until: Until) -> anyhow::Result<Vec<FoundDevice>>;
    fn open(&self, device: &str) -> anyhow::Result<Box<dyn Scanner>>;
}
