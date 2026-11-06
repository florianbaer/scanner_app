//! Job model and the shared, thread-safe job store.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::output::Format;
use crate::scanner::ScanSettings;

pub type JobId = u64;

/// Finished jobs kept in memory for display.
const MAX_FINISHED_JOBS: usize = 50;

/// Everything needed to run one scan batch.
#[derive(Debug, Clone)]
pub struct ScanRequest {
    pub device: String,
    pub settings: ScanSettings,
    pub format: Format,
    pub jpeg_quality: u8,
    /// File name prefix; sanitised and timestamped by the output layer.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Scanning,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Scanning => "scanning",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub request: ScanRequest,
    pub state: JobState,
    /// Pages scanned so far.
    pub pages: usize,
    /// Output file names (relative to the service's output directory).
    pub files: Vec<String>,
    pub error: Option<String>,
    pub created: SystemTime,
    pub finished: Option<SystemTime>,
    pub cancel_requested: bool,
}

#[derive(Default)]
pub(crate) struct JobStore {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Oldest first.
    jobs: VecDeque<Job>,
    next_id: JobId,
}

impl JobStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain data behind; keep serving it.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn create(&self, request: ScanRequest) -> JobId {
        let mut inner = self.lock();
        inner.next_id += 1;
        let id = inner.next_id;
        inner.jobs.push_back(Job {
            id,
            request,
            state: JobState::Queued,
            pages: 0,
            files: Vec::new(),
            error: None,
            created: SystemTime::now(),
            finished: None,
            cancel_requested: false,
        });
        Self::prune(&mut inner);
        id
    }

    pub fn get(&self, id: JobId) -> Option<Job> {
        self.lock().jobs.iter().find(|j| j.id == id).cloned()
    }

    /// All jobs, newest first.
    pub fn list(&self) -> Vec<Job> {
        self.lock().jobs.iter().rev().cloned().collect()
    }

    pub fn has_pending(&self) -> bool {
        self.lock().jobs.iter().any(|j| !j.state.is_finished())
    }

    pub fn update(&self, id: JobId, f: impl FnOnce(&mut Job)) {
        if let Some(job) = self.lock().jobs.iter_mut().find(|j| j.id == id) {
            f(job);
        }
    }

    pub fn is_cancel_requested(&self, id: JobId) -> bool {
        self.lock()
            .jobs
            .iter()
            .any(|j| j.id == id && j.cancel_requested)
    }

    /// Queued jobs are cancelled immediately; running jobs stop after the
    /// current page. Returns false if the job doesn't exist or already finished.
    pub fn request_cancel(&self, id: JobId) -> bool {
        let mut inner = self.lock();
        let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) else {
            return false;
        };
        match job.state {
            JobState::Queued => {
                job.state = JobState::Cancelled;
                job.finished = Some(SystemTime::now());
                true
            }
            JobState::Scanning => {
                job.cancel_requested = true;
                true
            }
            _ => false,
        }
    }

    fn prune(inner: &mut Inner) {
        while inner.jobs.iter().filter(|j| j.state.is_finished()).count() > MAX_FINISHED_JOBS {
            let Some(pos) = inner.jobs.iter().position(|j| j.state.is_finished()) else {
                break;
            };
            inner.jobs.remove(pos);
        }
    }
}
