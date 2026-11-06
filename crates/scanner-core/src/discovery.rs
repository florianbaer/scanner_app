//! Finding scanners. Several sources are queried in parallel and their results
//! merged. Concrete sources (SANE device list, mDNS) live with their backend.

use std::time::Duration;

use anyhow::Result;

pub use crate::scanner::{FoundDevice, Until};

/// A way of discovering scanners.
pub trait DeviceSource: Sync {
    fn label(&self) -> &'static str;
    fn discover(&self, timeout: Duration, until: Until) -> Result<Vec<FoundDevice>>;
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
