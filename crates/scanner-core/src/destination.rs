//! Destinations: where finished scan files are sent after they're written locally
//! (e.g. Paperless-ngx, S3). Implementations live outside core; core only defines
//! the contract and how deliveries are run and recorded.

use std::path::Path;
use std::sync::Arc;

/// Public description of a destination. Never contains secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationInfo {
    /// Stable id used to select the destination, e.g. `paperless`.
    pub id: String,
    /// Human-readable name, e.g. `Paperless (home)`.
    pub label: String,
    /// Implementation kind, e.g. `paperless`, `s3`.
    pub kind: &'static str,
}

/// Somewhere a finished scan file can be delivered to.
pub trait Destination: Send + Sync {
    fn info(&self) -> DestinationInfo;

    /// Uploads one file. Returns a short description of where it went
    /// (e.g. an object URL or a task id) for display.
    fn deliver(&self, file: &Path) -> anyhow::Result<String>;
}

pub type SharedDestination = Arc<dyn Destination>;

/// Outcome of delivering one file to one destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub destination: String,
    pub file: String,
    /// `Ok(location)` or `Err(message)`.
    pub result: Result<String, String>,
}

/// Looks up destinations by id, failing on the first unknown one.
pub fn select(
    available: &[SharedDestination],
    ids: &[String],
) -> Result<Vec<SharedDestination>, String> {
    ids.iter()
        .map(|id| {
            available
                .iter()
                .find(|d| d.info().id == *id)
                .cloned()
                .ok_or_else(|| format!("unknown destination '{id}'"))
        })
        .collect()
}

/// Delivers every file to every destination. Failures are recorded, not
/// propagated: one broken destination must not lose the others' uploads.
pub fn deliver_all(
    destinations: &[SharedDestination],
    dir: &Path,
    files: &[String],
) -> Vec<Delivery> {
    let mut out = Vec::new();
    for dest in destinations {
        let id = dest.info().id;
        for file in files {
            let result = dest.deliver(&dir.join(file)).map_err(|e| format!("{e:#}"));
            out.push(Delivery {
                destination: id.clone(),
                file: file.clone(),
                result,
            });
        }
    }
    out
}

/// Files that reached every destination. Only these may be removed locally.
pub fn fully_delivered(deliveries: &[Delivery], files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|f| {
            let mut for_file = deliveries.iter().filter(|d| &d.file == *f).peekable();
            for_file.peek().is_some() && for_file.all(|d| d.result.is_ok())
        })
        .cloned()
        .collect()
}

/// Result of [`deliver_files`].
#[derive(Debug, Clone, Default)]
pub struct DeliveryReport {
    pub deliveries: Vec<Delivery>,
    /// Files removed locally because every destination accepted them.
    pub deleted: Vec<String>,
}

/// Delivers `files` (in `dir`) to all `destinations`; with `delete_after`, removes
/// each local file that every destination accepted. Never fails as a whole:
/// problems are recorded in the report. Shared by the worker and the CLI.
pub fn deliver_files(
    destinations: &[SharedDestination],
    dir: &Path,
    files: &[String],
    delete_after: bool,
) -> DeliveryReport {
    let mut deliveries = deliver_all(destinations, dir, files);
    let mut deleted = Vec::new();
    if delete_after {
        for file in fully_delivered(&deliveries, files) {
            match std::fs::remove_file(dir.join(&file)) {
                Ok(()) => deleted.push(file),
                // Not fatal: the uploads succeeded; the local copy just stays.
                Err(e) => deliveries.push(Delivery {
                    destination: String::new(),
                    file: file.clone(),
                    result: Err(format!("uploaded, but could not delete local copy: {e}")),
                }),
            }
        }
    }
    DeliveryReport {
        deliveries,
        deleted,
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;

    /// Records delivered file names; fails if `fail` is set.
    pub struct Recorder {
        pub id: &'static str,
        pub fail: bool,
        pub delivered: Mutex<Vec<String>>,
    }

    impl Recorder {
        pub fn new(id: &'static str, fail: bool) -> Arc<Self> {
            Arc::new(Self {
                id,
                fail,
                delivered: Mutex::new(Vec::new()),
            })
        }
    }

    impl Destination for Recorder {
        fn info(&self) -> DestinationInfo {
            DestinationInfo {
                id: self.id.into(),
                label: self.id.into(),
                kind: "test",
            }
        }
        fn deliver(&self, file: &Path) -> anyhow::Result<String> {
            if self.fail {
                anyhow::bail!("upload refused");
            }
            assert!(
                file.is_file(),
                "delivered file must exist: {}",
                file.display()
            );
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            self.delivered.lock().unwrap().push(name.clone());
            Ok(format!("{}:{name}", self.id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Recorder;
    use super::*;

    fn delivery(file: &str, ok: bool) -> Delivery {
        Delivery {
            destination: "d".into(),
            file: file.into(),
            result: if ok {
                Ok(String::new())
            } else {
                Err(String::new())
            },
        }
    }

    #[test]
    fn only_files_delivered_everywhere_count() {
        let files = vec!["a".to_string(), "b".into(), "c".into()];
        let deliveries = [
            delivery("a", true),
            delivery("a", true),
            delivery("b", true),
            delivery("b", false),
        ];
        // c has no deliveries at all, so it must not be treated as delivered.
        assert_eq!(fully_delivered(&deliveries, &files), ["a"]);
    }

    #[test]
    fn select_rejects_unknown_ids() {
        let a: SharedDestination = Recorder::new("a", false);
        assert!(select(std::slice::from_ref(&a), &["a".into()]).is_ok());
        assert_eq!(
            select(&[a], &["nope".into()]).err().unwrap(),
            "unknown destination 'nope'"
        );
    }

    #[test]
    fn failures_are_recorded_and_others_still_delivered() {
        let dir = std::env::temp_dir().join(format!("dest_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.pdf"), b"%PDF").unwrap();
        let bad = Recorder::new("bad", true);
        let good = Recorder::new("good", false);
        let dests: Vec<SharedDestination> = vec![bad, good.clone()];
        let result = deliver_all(&dests, &dir, &["x.pdf".into()]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].result, Err("upload refused".into()));
        assert_eq!(result[1].result, Ok("good:x.pdf".into()));
        assert_eq!(*good.delivered.lock().unwrap(), ["x.pdf"]);
    }
}
