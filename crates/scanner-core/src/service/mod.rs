//! The scan service: the one entry point front-ends use to discover scanners,
//! queue scans and fetch results.
//!
//! ```text
//!  front-end ──submit/cancel/list──▶ JobStore ◀──progress── Worker ──▶ Backend
//!      │                                                      ▲
//!      └────────── commands (mpsc channel, in order) ─────────┘
//! ```
//!
//! A single worker thread owns the [`Backend`], so device access is serialised.
//! Job state lives in a shared [`JobStore`] that front-ends can read without
//! waiting for the worker. Failed uploads go to a persisted outbox that the
//! worker retries on a timer (any destination, survives restarts).

mod jobs;
mod outbox;
mod worker;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};

pub use jobs::{Job, JobId, JobState, ScanRequest};
pub use outbox::{PendingUpload, OUTBOX_FILE};

use crate::destination::{self, DestinationInfo, SharedDestination};
use crate::output::{self, StoredFile};
use crate::scanner::{Backend, Capabilities, FoundDevice};
use jobs::JobStore;
use outbox::Outbox;
use worker::{Command, Worker};

/// Expected service conditions a front-end may map to specific responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// The scanner is busy with a scan and the answer isn't cached.
    Busy,
    /// The request doesn't match what the scanner supports.
    Invalid(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Busy => write!(f, "the scanner is busy with another scan"),
            ServiceError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ServiceError {}

/// State shared between the service handle and the worker.
pub(crate) struct Shared {
    pub jobs: JobStore,
    pub output_dir: PathBuf,
    pub scanning: AtomicBool,
    pub devices: Mutex<Vec<FoundDevice>>,
    pub capabilities: Mutex<HashMap<String, Capabilities>>,
    pub destinations: Vec<SharedDestination>,
    pub outbox: Mutex<Outbox>,
    /// Last error persisting the outbox, if any (shown to users).
    pub outbox_error: Mutex<Option<String>>,
    pub retry_interval: Duration,
}

/// How the service is set up. Front-ends build this from their own config.
#[derive(Clone)]
pub struct ServiceConfig {
    /// Where scan files (and the pending-upload list) are written.
    pub output_dir: PathBuf,
    /// Targets scans may additionally be sent to.
    pub destinations: Vec<SharedDestination>,
    /// Wait between attempts for a failed upload.
    pub retry_interval: Duration,
    /// How often to look for uploads that are due.
    pub retry_check: Duration,
}

impl ServiceConfig {
    /// Defaults: no destinations, retry failed uploads every hour.
    pub fn new(output_dir: PathBuf) -> Self {
        Self {
            output_dir,
            destinations: Vec::new(),
            retry_interval: Duration::from_secs(60 * 60),
            retry_check: Duration::from_secs(60),
        }
    }
}

/// Result of a discovery request.
#[derive(Debug, Clone)]
pub struct DeviceList {
    pub devices: Vec<FoundDevice>,
    /// True if the scanner was busy and this is the last known list.
    pub cached: bool,
}

/// Cheap to share (`Arc` it); all methods take `&self`.
pub struct ScannerService {
    tx: Sender<Command>,
    shared: Arc<Shared>,
}

impl ScannerService {
    /// Starts the worker thread and the retry timer. Fails if the persisted
    /// pending-upload list can't be read.
    pub fn start(backend: impl Backend, config: ServiceConfig) -> Result<Self> {
        let outbox = Outbox::load(&config.output_dir)?;
        let shared = Arc::new(Shared {
            jobs: JobStore::default(),
            output_dir: config.output_dir,
            scanning: AtomicBool::new(false),
            devices: Mutex::new(Vec::new()),
            capabilities: Mutex::new(HashMap::new()),
            destinations: config.destinations,
            outbox: Mutex::new(outbox),
            outbox_error: Mutex::new(None),
            retry_interval: config.retry_interval,
        });
        let (tx, rx) = mpsc::channel();
        let worker = Worker {
            backend,
            shared: shared.clone(),
        };
        std::thread::Builder::new()
            .name("scanner-worker".into())
            .spawn(move || worker.run(rx))?;

        // Retry timer: asks the worker to process due uploads, in order with scans.
        let ticker = tx.clone();
        let every = config.retry_check;
        std::thread::Builder::new()
            .name("scanner-retry-timer".into())
            .spawn(move || loop {
                std::thread::sleep(every);
                if ticker.send(Command::RetryDue).is_err() {
                    break;
                }
            })?;
        // Pick up anything left over from before a restart right away.
        let _ = tx.send(Command::RetryDue);

        Ok(Self { tx, shared })
    }

    pub fn output_dir(&self) -> &std::path::Path {
        &self.shared.output_dir
    }

    /// True while a job is queued or scanning.
    pub fn is_busy(&self) -> bool {
        self.shared.jobs.has_pending() || self.shared.scanning.load(Ordering::SeqCst)
    }

    /// Finds scanners. Blocks for up to `timeout`; while a scan is running it
    /// returns the last known list immediately instead.
    pub fn discover(&self, timeout: Duration) -> Result<DeviceList> {
        if self.is_busy() {
            return Ok(DeviceList {
                devices: lock(&self.shared.devices).clone(),
                cached: true,
            });
        }
        let devices = self.ask(|reply| Command::Discover { timeout, reply })?;
        Ok(DeviceList {
            devices,
            cached: false,
        })
    }

    /// What `device` supports. Cached per device; blocks on first use.
    pub fn capabilities(&self, device: &str) -> Result<Capabilities> {
        if let Some(caps) = lock(&self.shared.capabilities).get(device) {
            return Ok(caps.clone());
        }
        if self.is_busy() {
            return Err(ServiceError::Busy.into());
        }
        self.ask(|reply| Command::Capabilities {
            device: device.to_string(),
            reply,
        })
    }

    /// Queues a scan and returns its job id. Validated against cached
    /// capabilities when available; the backend validates the rest.
    pub fn submit(&self, request: ScanRequest) -> Result<JobId> {
        if request.device.trim().is_empty() {
            return Err(ServiceError::Invalid("no scanner selected".into()).into());
        }
        if !(1..=100).contains(&request.jpeg_quality) {
            return Err(ServiceError::Invalid("quality must be between 1 and 100".into()).into());
        }
        destination::select(&self.shared.destinations, &request.destinations)
            .map_err(ServiceError::Invalid)?;
        if request.delete_after_delivery && request.destinations.is_empty() {
            return Err(ServiceError::Invalid(
                "delete after upload needs at least one destination".into(),
            )
            .into());
        }
        if let Some(caps) = lock(&self.shared.capabilities).get(&request.device) {
            request
                .settings
                .validate(caps)
                .map_err(ServiceError::Invalid)?;
        }
        let id = self.shared.jobs.create(request);
        self.tx
            .send(Command::Scan(id))
            .map_err(|_| anyhow!("scan worker has stopped"))?;
        Ok(id)
    }

    /// Configured destinations (ids and labels only, no secrets).
    pub fn destinations(&self) -> Vec<DestinationInfo> {
        self.shared.destinations.iter().map(|d| d.info()).collect()
    }

    /// Uploads waiting for a retry, oldest first.
    pub fn pending_uploads(&self) -> Vec<PendingUpload> {
        lock(&self.shared.outbox).items().to_vec()
    }

    /// Problem persisting the pending-upload list, if any.
    pub fn pending_uploads_error(&self) -> Option<String> {
        lock(&self.shared.outbox_error).clone()
    }

    /// Retries all pending uploads now instead of waiting for their next slot.
    pub fn retry_uploads_now(&self) -> Result<()> {
        lock(&self.shared.outbox).make_all_due(outbox::now_secs())?;
        self.tx
            .send(Command::RetryDue)
            .map_err(|_| anyhow!("scan worker has stopped"))
    }

    /// All recent jobs, newest first.
    pub fn jobs(&self) -> Vec<Job> {
        self.shared.jobs.list()
    }

    pub fn job(&self, id: JobId) -> Option<Job> {
        self.shared.jobs.get(id)
    }

    /// Cancels a queued job, or stops a running one after the current page
    /// (pages already scanned are kept).
    pub fn cancel(&self, id: JobId) -> bool {
        self.shared.jobs.request_cancel(id)
    }

    /// Finished scan files, newest first.
    pub fn files(&self) -> Result<Vec<StoredFile>> {
        output::list_files(&self.shared.output_dir)
    }

    /// Path of a scan file for download, or `None` for unknown/unsafe names.
    pub fn file_path(&self, name: &str) -> Option<PathBuf> {
        output::resolve(&self.shared.output_dir, name)
    }

    fn ask<T>(&self, make: impl FnOnce(Sender<Result<T>>) -> Command) -> Result<T> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(make(reply))
            .map_err(|_| anyhow!("scan worker has stopped"))?;
        rx.recv().map_err(|_| anyhow!("scan worker has stopped"))?
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests;
