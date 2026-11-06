//! The worker: a single thread that owns the backend and processes commands in
//! order. This serialises all device access; scanners do one thing at a time.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;

use super::jobs::{JobId, JobState, ScanRequest};
use super::outbox::{now_secs, PendingUpload};
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
    /// Retry pending uploads whose time has come.
    RetryDue,
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
                Command::RetryDue => self.retry_due(),
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

        self.enqueue_failures(&report, job.request.delete_after_delivery);

        jobs.update(id, |j| {
            j.files = files;
            j.deliveries = report.deliveries;
            j.deleted_files = report.deleted;
            j.finished = Some(SystemTime::now());
            j.state = state;
            j.error = error.map(|e| format!("{e:#}"));
        });
    }

    /// Puts failed uploads into the outbox for later retries.
    fn enqueue_failures(&self, report: &DeliveryReport, delete_after: bool) {
        let now = now_secs();
        let next = now + self.shared.retry_interval.as_secs();
        for d in &report.deliveries {
            // Entries without a destination are local problems (e.g. couldn't
            // delete a file), not uploads to retry.
            let Err(error) = &d.result else { continue };
            if d.destination.is_empty() || d.file.is_empty() {
                continue;
            }
            let item = PendingUpload {
                destination: d.destination.clone(),
                file: d.file.clone(),
                attempts: 1,
                last_error: error.clone(),
                created: now,
                next_attempt: next,
                delete_after,
            };
            let result = self.outbox().add(item);
            self.note_outbox(result);
        }
    }

    /// Retries due uploads. The outbox lock is not held during uploads.
    fn retry_due(&self) {
        let now = now_secs();
        let due = self.outbox().due(now);
        for item in due {
            let path = self.shared.output_dir.join(&item.file);
            let dest = self
                .shared
                .destinations
                .iter()
                .find(|d| d.info().id == item.destination);
            // Nothing left to send: the file was removed, or the destination
            // is no longer configured.
            let (Some(dest), true) = (dest, path.is_file()) else {
                let r = self.outbox().remove(&item.destination, &item.file);
                self.note_outbox(r);
                continue;
            };
            match dest.deliver(&path) {
                Ok(_) => {
                    let mut outbox = self.outbox();
                    let r = outbox.remove(&item.destination, &item.file);
                    let delete = item.delete_after && !outbox.has_file(&item.file);
                    drop(outbox);
                    self.note_outbox(r);
                    if delete {
                        // Every destination has it now; the local copy may go.
                        let _ = std::fs::remove_file(&path);
                    }
                }
                Err(e) => {
                    let next = now_secs() + self.shared.retry_interval.as_secs();
                    let r = self.outbox().reschedule(
                        &item.destination,
                        &item.file,
                        format!("{e:#}"),
                        next,
                    );
                    self.note_outbox(r);
                }
            }
        }
    }

    fn outbox(&self) -> std::sync::MutexGuard<'_, super::outbox::Outbox> {
        self.shared.outbox.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Remembers whether persisting the outbox worked, for display.
    fn note_outbox(&self, result: Result<()>) {
        *self
            .shared
            .outbox_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = result.err().map(|e| format!("{e:#}"));
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
