//! Finding scanners on the network. Several sources are queried in parallel and
//! their results merged.

use std::time::{Duration, Instant};

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent};

use crate::sane::Sane;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundDevice {
    /// Device name to open with the backend, e.g. `epsonds:net:192.0.2.10`.
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

/// A way of discovering scanners.
pub trait DeviceSource: Sync {
    fn label(&self) -> &'static str;
    fn discover(&self, timeout: Duration, until: Until) -> Result<Vec<FoundDevice>>;
}

/// Asks SANE (USB devices and SANE's own network autodiscovery).
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

/// Result of querying all sources. Errors from individual sources don't fail the
/// whole discovery; they're returned for the front-end to report.
pub struct Discovery {
    pub devices: Vec<FoundDevice>,
    pub errors: Vec<(&'static str, anyhow::Error)>,
}

/// Queries all sources in parallel and merges the results (deduplicated by name,
/// in source order).
pub fn discover(sources: &[&dyn DeviceSource], timeout: Duration, until: Until) -> Discovery {
    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = sources
            .iter()
            .map(|src| s.spawn(move || (src.label(), src.discover(timeout, until))))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let mut out = Discovery {
        devices: Vec::new(),
        errors: Vec::new(),
    };
    for (label, result) in results {
        match result {
            Ok(devs) => {
                for d in devs {
                    if !out.devices.iter().any(|e| e.name == d.name) {
                        out.devices.push(d);
                    }
                }
            }
            Err(e) => out.errors.push((label, e)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(&'static str, Vec<&'static str>);

    impl DeviceSource for Fixed {
        fn label(&self) -> &'static str {
            self.0
        }
        fn discover(&self, _: Duration, _: Until) -> Result<Vec<FoundDevice>> {
            if self.1.is_empty() {
                anyhow::bail!("boom");
            }
            Ok(self
                .1
                .iter()
                .map(|n| FoundDevice {
                    name: n.to_string(),
                    label: String::new(),
                })
                .collect())
        }
    }

    #[test]
    fn merges_dedupes_and_collects_errors() {
        let a = Fixed("a", vec!["x", "y"]);
        let b = Fixed("b", vec!["y", "z"]);
        let c = Fixed("c", vec![]);
        let d = discover(&[&a, &b, &c], Duration::from_millis(1), Until::Timeout);
        let names: Vec<_> = d.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["x", "y", "z"]);
        assert_eq!(d.errors.len(), 1);
        assert_eq!(d.errors[0].0, "c");
    }
}
