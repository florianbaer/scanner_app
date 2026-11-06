//! The worker: a single thread that owns the backend and processes commands in
//! order. This serialises all device access; scanners do one thing at a time.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;

use super::jobs::{JobId, JobState, ScanRequest};
use super::Shared;
use crate::destination::{self, deliver_files, DeliveryReport};
use crate::output::{create_sink, OutputSpec};
use crate::scanner::{Backend, Capabilities, FoundDevice, ScanError, Until};

pub(crate) enum Command {
    Discover {
        timeout: Duration,
        reply: Sender<Result<Vec<FoundDevice>>>,
    },
    Capabilities {
        device: String,
        reply: Sender<Result<Capabilities>>,
    },
    Scan(JobId),
}

pub(crate) struct Worker<B: Backend> {
    pub backend: B,
    pub shared: Arc<Shared>,
}

impl<B: Backend> Worker<B> {
    /// Runs until every sender is dropped.
    pub fn run(self, rx: Receiver<Command>) {
        for command in rx {
            match command {
                Command::Discover { timeout, reply } => {
                    let result = self.backend.discover(timeout, Until::Timeout);
                    if let Ok(devices) = &result {
                        *self
                            .shared
                            .devices
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = devices.clone();
                    }
                    let _ = reply.send(result);
                }
                Command::Capabilities { device, reply } => {
                    let result = self.backend.open(&device).map(|s| s.capabilities());
                    if let Ok(caps) = &result {
                        self.shared
                            .capabilities
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(device, caps.clone());
                    }
                    let _ = reply.send(result);
                }
                Command::Scan(id) => self.run_job(id),
            }
        }
    }

    fn run_job(&self, id: JobId) {
        let jobs = &self.shared.jobs;
        let Some(job) = jobs.get(id) else { return };
        if job.state != JobState::Queued {
            return; // cancelled while waiting
        }
        jobs.update(id, |j| j.state = JobState::Scanning);
        self.shared.scanning.store(true, Ordering::SeqCst);

        let (files, result) = self.scan(id, &job.request);

        self.shared.scanning.store(false, Ordering::SeqCst);

        let state = match &result {
            Ok(()) => JobState::Done,
            Err(e) if e.downcast_ref::<ScanError>() == Some(&ScanError::Cancelled) => {
                JobState::Cancelled
            }
            Err(_) => JobState::Failed,
        };
        let error = result.err().filter(|_| state == JobState::Failed);

        // Only complete batches leave the machine; partial ones stay local.
        let mut report = DeliveryReport::default();
        if state == JobState::Done && !job.request.destinations.is_empty() {
            jobs.update(id, |j| {
                j.files = files.clone();
                j.state = JobState::Delivering;
            });
            // Ids were validated on submit; a missing one is reported, not skipped.
            match destination::select(&self.shared.destinations, &job.request.destinations) {
                Ok(dests) => {
                    report = deliver_files(
                        &dests,
                        &self.shared.output_dir,
                        &files,
                        job.request.delete_after_delivery,
                    );
                }
                Err(msg) => report.deliveries.push(destination::Delivery {
                    destination: String::new(),
                    file: String::new(),
                    result: Err(msg),
                }),
            }
        }

        jobs.update(id, |j| {
            j.files = files;
            j.deliveries = report.deliveries;
            j.deleted_files = report.deleted;
            j.finished = Some(SystemTime::now());
            j.state = state;
            j.error = error.map(|e| format!("{e:#}"));
        });
    }

    /// Returns the files written (kept even if the batch failed midway) and the outcome.
    fn scan(&self, id: JobId, req: &ScanRequest) -> (Vec<String>, Result<()>) {
        let jobs = &self.shared.jobs;
        let spec = OutputSpec::timestamped(
            self.shared.output_dir.clone(),
            &req.name,
            req.format,
            req.jpeg_quality,
        );
        let mut sink = match create_sink(&spec) {
            Ok(s) => s,
            Err(e) => return (Vec::new(), Err(e)),
        };

        let result = self.backend.open(&req.device).and_then(|mut scanner| {
            scanner.scan(&req.settings, &mut |page| {
                if jobs.is_cancel_requested(id) {
                    return Err(ScanError::Cancelled.into());
                }
                sink.add(&page)?;
                jobs.update(id, |j| j.pages += 1);
                Ok(())
            })
            // The scanner is dropped (device released) here, before writing output.
        });

        let files = match sink.finish() {
            Ok(paths) => paths
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect(),
            Err(e) => return (Vec::new(), result.and(Err(e))),
        };
        (files, result.map(|_| ()))
    }
}
