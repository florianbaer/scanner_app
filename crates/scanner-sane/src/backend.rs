//! SANE as a `Backend`: discovery (SANE device list + mDNS) and opening devices.

use std::time::Duration;

use anyhow::Result;

use scanner_core::discovery::{self, DeviceSource};
use scanner_core::scanner::{Backend, FoundDevice, Scanner, Until};

use crate::{MdnsEpsonSource, Sane, SaneScanner};

/// Asks SANE for devices (USB and SANE's own network autodiscovery).
pub struct SaneSource(pub Sane);

impl DeviceSource for SaneSource {
    fn label(&self) -> &'static str {
        "SANE"
    }

    fn discover(&self, _timeout: Duration, _until: Until) -> Result<Vec<FoundDevice>> {
        Ok(self
            .0
            .devices()?
            .into_iter()
            .map(|d| FoundDevice {
                label: format!("{} {}", d.vendor, d.model).trim().to_string(),
                name: d.name,
            })
            .collect())
    }
}

pub struct SaneBackend {
    sane: Sane,
}

impl SaneBackend {
    pub fn new() -> Result<Self> {
        Ok(Self {
            sane: Sane::init()?,
        })
    }

    /// Opens the concrete SANE scanner, e.g. for raw option access.
    pub fn open_sane(&self, device: &str) -> Result<SaneScanner> {
        SaneScanner::open(&self.sane, device)
    }

    /// Discovery that also reports per-source failures (for diagnostics).
    pub fn discover_detailed(&self, timeout: Duration, until: Until) -> discovery::Discovery {
        let sane_src = SaneSource(self.sane.clone());
        discovery::discover(&[&MdnsEpsonSource, &sane_src], timeout, until)
    }
}

impl Backend for SaneBackend {
    fn discover(&self, timeout: Duration, until: Until) -> Result<Vec<FoundDevice>> {
        let mut result = self.discover_detailed(timeout, until);
        // Only fail if nothing was found *and* a source broke.
        if result.devices.is_empty() && !result.errors.is_empty() {
            let (source, err) = result.errors.remove(0);
            return Err(err.context(format!("{source} discovery failed")));
        }
        Ok(result.devices)
    }

    fn open(&self, device: &str) -> Result<Box<dyn Scanner>> {
        Ok(Box::new(self.open_sane(device)?))
    }
}
