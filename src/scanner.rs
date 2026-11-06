//! Domain model for scanning, independent of any backend or front-end.

use std::fmt;

use image::DynamicImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Color,
    Gray,
    Lineart,
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

/// Expected conditions a front-end may want to handle specially.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanError {
    FeederEmpty,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::FeederEmpty => write!(f, "no documents in the feeder"),
        }
    }
}

impl std::error::Error for ScanError {}

/// A device that can scan a batch of sheets from its feeder.
pub trait Scanner {
    /// Scans until the feeder is empty, handing each page to `on_page` as it arrives.
    /// Returns the number of pages scanned.
    fn scan(
        &mut self,
        settings: &ScanSettings,
        on_page: &mut dyn FnMut(Page) -> anyhow::Result<()>,
    ) -> anyhow::Result<usize>;
}
