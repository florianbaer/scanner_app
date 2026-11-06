//! Implements the domain `Scanner` trait on top of a SANE device. This is the only
//! place that knows SANE option names like `source` or `adf-crp`.

use anyhow::{Context, Result};

use crate::{Constraint, Device, Sane};
use scanner_core::raster;
use scanner_core::scanner::{
    Capabilities, ColorMode, Page, ScanError, ScanSettings, Scanner, Side, Source,
};

pub struct SaneScanner {
    device: Device,
    name: String,
}

impl SaneScanner {
    pub fn open(sane: &Sane, name: &str) -> Result<Self> {
        let device = sane.open(name)?;
        Ok(Self {
            device,
            name: name.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Access to raw SANE options, e.g. for diagnostics.
    pub fn device(&self) -> &Device {
        &self.device
    }

    fn apply(&self, s: &ScanSettings) -> Result<u32> {
        let d = &self.device;
        // Order matters: changing source/mode can reload other options.
        d.set_str(
            "source",
            match s.source {
                Source::Feeder => "ADF Front",
                Source::FeederDuplex => "ADF Duplex",
            },
        )?;
        d.set_str(
            "mode",
            match s.mode {
                ColorMode::Color => "Color",
                ColorMode::Gray => "Gray",
                ColorMode::Lineart => "Lineart",
            },
        )?;
        d.set_str("resolution", &s.dpi.to_string())?;
        // Feature options are model-specific; skip them where they don't exist.
        for (name, on) in [("adf-crp", s.auto_crop), ("adf-skew", s.deskew)] {
            if d.find(name).is_some_and(|o| o.is_active()) {
                d.set_str(name, if on { "yes" } else { "no" })?;
            }
        }
        for (k, v) in &s.extra_options {
            d.set_str(k, v)?;
        }

        let dpi = d
            .find("resolution")
            .and_then(|o| d.get(&o).ok())
            .and_then(|v| v.as_f64())
            .map(|x| x.round() as u32)
            .unwrap_or(s.dpi);
        Ok(dpi)
    }

    fn scan_inner(
        &self,
        s: &ScanSettings,
        on_page: &mut dyn FnMut(Page) -> Result<()>,
    ) -> Result<usize> {
        let dpi = self.apply(s)?;
        let duplex = s.source == Source::FeederDuplex;
        let mut count = 0usize;
        loop {
            match self.device.start() {
                Ok(()) => {}
                Err(e) if e.is_no_docs() => break,
                Err(e) => return Err(e.into()),
            }
            // Per the SANE standard, query parameters after start and before reading.
            // Some backends (epsonds) only fetch the image inside this call, so reading
            // first yields nothing, and querying again afterwards may start the next page.
            // Height is derived from the data length, so an unknown line count is fine.
            let format = self.device.raster_format()?;
            let data = self.device.read_frame()?;
            let image = raster::decode(&format, &data).context("decoding scanned image")?;
            let (sheet, side) = if duplex {
                (
                    count / 2 + 1,
                    if count.is_multiple_of(2) {
                        Side::Front
                    } else {
                        Side::Back
                    },
                )
            } else {
                (count + 1, Side::Single)
            };
            on_page(Page {
                sheet,
                side,
                dpi,
                image,
            })?;
            count += 1;
        }
        if count == 0 {
            return Err(ScanError::FeederEmpty.into());
        }
        Ok(count)
    }
}

/// Resolutions offered when a backend only reports a range.
const COMMON_DPI: [u32; 8] = [75, 100, 150, 200, 300, 400, 600, 1200];

impl Scanner for SaneScanner {
    fn capabilities(&self) -> Capabilities {
        let d = &self.device;
        let constraint = |name: &str| d.find(name).map(|o| o.constraint);
        let resolutions = match constraint("resolution") {
            Some(Constraint::List(v)) => v.iter().map(|x| x.round() as u32).collect(),
            Some(Constraint::Range { min, max, .. }) => COMMON_DPI
                .into_iter()
                .filter(|&r| (min..=max).contains(&(r as f64)))
                .collect(),
            _ => Vec::new(),
        };
        let modes = match constraint("mode") {
            Some(Constraint::Strings(v)) => v.iter().filter_map(|m| m.parse().ok()).collect(),
            _ => Vec::new(),
        };
        let duplex = matches!(constraint("source"),
            Some(Constraint::Strings(v)) if v.iter().any(|s| s.to_ascii_lowercase().contains("duplex")));
        Capabilities {
            resolutions,
            modes,
            duplex,
            auto_crop: d.find("adf-crp").is_some(),
            deskew: d.find("adf-skew").is_some(),
        }
    }

    fn scan(
        &mut self,
        settings: &ScanSettings,
        on_page: &mut dyn FnMut(Page) -> Result<()>,
    ) -> Result<usize> {
        let result = self.scan_inner(settings, on_page);
        // Always end the batch so the scanner is ready for the next one.
        self.device.cancel();
        result
    }
}
