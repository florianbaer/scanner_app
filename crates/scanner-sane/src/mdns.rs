//! Bonjour discovery of Epson network scanners.

use std::time::{Duration, Instant};

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use scanner_core::discovery::DeviceSource;
use scanner_core::scanner::{FoundDevice, Until};

/// Browses Bonjour for Epson network scanners (`_scanner._tcp`) and maps them to
/// SANE `epsonds:net:<ip>` device names.
pub struct MdnsEpsonSource;

const SCANNER_SERVICE: &str = "_scanner._tcp.local.";

impl DeviceSource for MdnsEpsonSource {
    fn label(&self) -> &'static str {
        "mDNS"
    }

    fn discover(&self, timeout: Duration, until: Until) -> Result<Vec<FoundDevice>> {
        let daemon = ServiceDaemon::new()?;
        let rx = daemon.browse(SCANNER_SERVICE)?;
        let deadline = Instant::now() + timeout;
        let mut found = Vec::new();

        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Ok(event) = rx.recv_timeout(left) else {
                break;
            };
            let ServiceEvent::ServiceResolved(info) = event else {
                continue;
            };
            let is_epson = info
                .get_property_val_str("mfg")
                .is_some_and(|m| m.eq_ignore_ascii_case("EPSON"));
            if !is_epson {
                continue;
            }
            let Some(ip) = info.get_addresses_v4().into_iter().min() else {
                continue;
            };
            let label = info
                .get_property_val_str("ty")
                .map(str::to_string)
                .unwrap_or_else(|| {
                    info.get_fullname()
                        .trim_end_matches(SCANNER_SERVICE)
                        .trim_end_matches('.')
                        .to_string()
                });
            let dev = FoundDevice {
                name: format!("epsonds:net:{ip}"),
                label,
            };
            if !found.contains(&dev) {
                found.push(dev);
            }
            if until == Until::First {
                break;
            }
        }

        let _ = daemon.shutdown();
        Ok(found)
    }
}
