//! The outbox: uploads that failed and will be retried, persisted next to the
//! scans so retries survive restarts. Works for any destination.
//!
//! Only the worker mutates it (plus "retry now", which just moves due times).
//! Every change is written atomically (temp file + rename).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// File name inside the output directory. Hidden (dot) files never show up in
/// the scan file list.
pub const OUTBOX_FILE: &str = ".pending-uploads.json";

/// One file waiting to be uploaded to one destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpload {
    pub destination: String,
    pub file: String,
    /// Failed attempts so far (the first, immediate attempt counts).
    pub attempts: u32,
    pub last_error: String,
    /// Unix seconds.
    pub created: u64,
    /// Unix seconds; due when `<= now`.
    pub next_attempt: u64,
    /// Delete the local file once no upload for it is pending any more.
    pub delete_after: bool,
}

pub(crate) struct Outbox {
    path: PathBuf,
    items: Vec<PendingUpload>,
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl Outbox {
    /// Loads the outbox in `dir`; a missing file is an empty outbox.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(OUTBOX_FILE);
        let items = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("reading pending uploads from {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", path.display()));
            }
        };
        Ok(Self { path, items })
    }

    pub fn items(&self) -> &[PendingUpload] {
        &self.items
    }

    /// Entries due at `now`.
    pub fn due(&self, now: u64) -> Vec<PendingUpload> {
        self.items
            .iter()
            .filter(|i| i.next_attempt <= now)
            .cloned()
            .collect()
    }

    pub fn has_file(&self, file: &str) -> bool {
        self.items.iter().any(|i| i.file == file)
    }

    /// Adds an entry, replacing one for the same destination and file.
    pub fn add(&mut self, item: PendingUpload) -> Result<()> {
        self.items
            .retain(|i| !(i.destination == item.destination && i.file == item.file));
        self.items.push(item);
        self.save()
    }

    pub fn remove(&mut self, destination: &str, file: &str) -> Result<()> {
        self.items
            .retain(|i| !(i.destination == destination && i.file == file));
        self.save()
    }

    /// Records another failed attempt and schedules the next one.
    pub fn reschedule(
        &mut self,
        destination: &str,
        file: &str,
        error: String,
        next: u64,
    ) -> Result<()> {
        if let Some(i) = self
            .items
            .iter_mut()
            .find(|i| i.destination == destination && i.file == file)
        {
            i.attempts += 1;
            i.last_error = error;
            i.next_attempt = next;
        }
        self.save()
    }

    /// Makes every entry due now ("retry now").
    pub fn make_all_due(&mut self, now: u64) -> Result<()> {
        for i in &mut self.items {
            i.next_attempt = i.next_attempt.min(now);
        }
        self.save()
    }

    fn save(&self) -> Result<()> {
        if self.items.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    Err(e).with_context(|| format!("removing {}", self.path.display()))
                }
                _ => Ok(()),
            };
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.items)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("writing {}", self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(dest: &str, file: &str, next: u64) -> PendingUpload {
        PendingUpload {
            destination: dest.into(),
            file: file.into(),
            attempts: 1,
            last_error: "down".into(),
            created: 0,
            next_attempt: next,
            delete_after: false,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("outbox_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn persists_across_reload_and_cleans_up_when_empty() {
        let dir = temp_dir("persist");
        let mut o = Outbox::load(&dir).unwrap();
        o.add(item("p", "a.pdf", 100)).unwrap();
        o.add(item("s3", "a.pdf", 50)).unwrap();
        o.add(item("p", "a.pdf", 200)).unwrap(); // replaces the first

        let mut o = Outbox::load(&dir).unwrap();
        assert_eq!(o.items().len(), 2);
        assert_eq!(o.due(150).len(), 1, "only s3 is due at 150");
        assert!(o.has_file("a.pdf"));

        o.reschedule("s3", "a.pdf", "still down".into(), 999)
            .unwrap();
        let s3 = o.items().iter().find(|i| i.destination == "s3").unwrap();
        assert_eq!(
            (s3.attempts, s3.next_attempt, s3.last_error.as_str()),
            (2, 999, "still down")
        );

        o.make_all_due(10).unwrap();
        assert_eq!(o.due(10).len(), 2);

        o.remove("p", "a.pdf").unwrap();
        o.remove("s3", "a.pdf").unwrap();
        assert!(!dir.join(OUTBOX_FILE).exists(), "file removed when empty");
        assert!(Outbox::load(&dir).unwrap().items().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
