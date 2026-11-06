//! Service tests against a fake backend: no hardware needed.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::{DynamicImage, RgbImage};

use super::*;
use crate::destination::testing::Recorder;
use crate::destination::SharedDestination;
use crate::output::Format;
use crate::scanner::{
    Backend, Capabilities, ColorMode, Page, ScanError, ScanSettings, Scanner, Side, Source, Until,
};

#[derive(Clone)]
struct FakeBackend {
    pages: usize,
    page_delay: Duration,
}

struct FakeScanner(FakeBackend);

impl Backend for FakeBackend {
    fn discover(&self, _: Duration, _: Until) -> Result<Vec<FoundDevice>> {
        Ok(vec![FoundDevice {
            name: "fake:0".into(),
            label: "Fake Scanner".into(),
        }])
    }
    fn open(&self, _: &str) -> Result<Box<dyn Scanner>> {
        Ok(Box::new(FakeScanner(self.clone())))
    }
}

impl Scanner for FakeScanner {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            resolutions: vec![150, 300],
            modes: vec![ColorMode::Color, ColorMode::Gray],
            duplex: false,
            auto_crop: true,
            deskew: true,
        }
    }
    fn scan(
        &mut self,
        _: &ScanSettings,
        on_page: &mut dyn FnMut(Page) -> Result<()>,
    ) -> Result<usize> {
        if self.0.pages == 0 {
            return Err(ScanError::FeederEmpty.into());
        }
        for i in 0..self.0.pages {
            std::thread::sleep(self.0.page_delay);
            on_page(Page {
                sheet: i + 1,
                side: Side::Single,
                dpi: 300,
                image: DynamicImage::ImageRgb8(RgbImage::new(4, 4)),
            })?;
        }
        Ok(self.0.pages)
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "scanner_core_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn service(pages: usize, delay_ms: u64, dir: &TempDir) -> ScannerService {
    service_with(pages, delay_ms, dir, Vec::new())
}

fn service_with(
    pages: usize,
    delay_ms: u64,
    dir: &TempDir,
    destinations: Vec<SharedDestination>,
) -> ScannerService {
    let backend = FakeBackend {
        pages,
        page_delay: Duration::from_millis(delay_ms),
    };
    let mut config = ServiceConfig::new(dir.0.clone());
    config.destinations = destinations;
    ScannerService::start(backend, config).unwrap()
}

/// Service whose retries happen almost immediately, for retry tests.
fn fast_retry_service(dir: &TempDir, destinations: Vec<SharedDestination>) -> ScannerService {
    let backend = FakeBackend {
        pages: 1,
        page_delay: Duration::ZERO,
    };
    let mut config = ServiceConfig::new(dir.0.clone());
    config.destinations = destinations;
    config.retry_interval = Duration::ZERO;
    config.retry_check = Duration::from_millis(20);
    ScannerService::start(backend, config).unwrap()
}

/// Fails the first `failures` uploads, then succeeds.
struct Flaky {
    failures: std::sync::atomic::AtomicUsize,
    delivered: std::sync::atomic::AtomicUsize,
}

impl Flaky {
    fn new(failures: usize) -> Arc<Self> {
        Arc::new(Self {
            failures: failures.into(),
            delivered: 0.into(),
        })
    }
}

impl crate::destination::Destination for Flaky {
    fn info(&self) -> crate::destination::DestinationInfo {
        crate::destination::DestinationInfo {
            id: "flaky".into(),
            label: "Flaky".into(),
            kind: "test",
        }
    }
    fn deliver(&self, _: &std::path::Path) -> Result<String> {
        use std::sync::atomic::Ordering::SeqCst;
        if self
            .failures
            .fetch_update(SeqCst, SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            anyhow::bail!("temporarily unavailable");
        }
        self.delivered.fetch_add(1, SeqCst);
        Ok("ok".into())
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn request() -> ScanRequest {
    ScanRequest {
        device: "fake:0".into(),
        settings: ScanSettings::default(),
        format: Format::Pdf,
        jpeg_quality: 80,
        name: "test".into(),
        destinations: Vec::new(),
        delete_after_delivery: false,
    }
}

fn wait_finished(svc: &ScannerService, id: JobId) -> Job {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let job = svc.job(id).unwrap();
        if job.state.is_finished() {
            return job;
        }
        assert!(Instant::now() < deadline, "job did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn scan_job_writes_pdf_and_lists_it() {
    let dir = TempDir::new("ok");
    let svc = service(3, 0, &dir);
    let id = svc.submit(request()).unwrap();
    let job = wait_finished(&svc, id);
    assert_eq!(job.state, JobState::Done, "{:?}", job.error);
    assert_eq!(job.pages, 3);
    assert_eq!(job.files.len(), 1);
    assert!(job.files[0].starts_with("test_") && job.files[0].ends_with(".pdf"));
    let files = svc.files().unwrap();
    assert_eq!(files[0].name, job.files[0]);
    assert!(svc.file_path(&job.files[0]).is_some());
    assert!(!svc.is_busy());
}

#[test]
fn empty_feeder_fails_with_message() {
    let dir = TempDir::new("empty");
    let svc = service(0, 0, &dir);
    let job = wait_finished(&svc, svc.submit(request()).unwrap());
    assert_eq!(job.state, JobState::Failed);
    assert!(job.error.unwrap().contains("no documents"));
}

#[test]
fn cancel_stops_after_current_page_and_keeps_pages() {
    let dir = TempDir::new("cancel");
    let svc = service(50, 20, &dir);
    let id = svc.submit(request()).unwrap();
    while svc.job(id).unwrap().pages == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(svc.cancel(id));
    let job = wait_finished(&svc, id);
    assert_eq!(job.state, JobState::Cancelled);
    assert!(job.pages > 0 && job.pages < 50);
    assert_eq!(job.files.len(), 1, "scanned pages are kept");
}

#[test]
fn queued_job_can_be_cancelled_before_it_runs() {
    let dir = TempDir::new("queued");
    let svc = service(5, 20, &dir);
    let first = svc.submit(request()).unwrap();
    let second = svc.submit(request()).unwrap();
    assert!(svc.cancel(second));
    assert_eq!(wait_finished(&svc, second).state, JobState::Cancelled);
    assert_eq!(wait_finished(&svc, first).state, JobState::Done);
    assert_eq!(svc.job(second).unwrap().pages, 0);
}

#[test]
fn discovery_and_capabilities_validate_requests() {
    let dir = TempDir::new("caps");
    let svc = service(1, 0, &dir);
    let list = svc.discover(Duration::from_millis(1)).unwrap();
    assert_eq!(list.devices[0].name, "fake:0");
    assert!(!list.cached);
    let caps = svc.capabilities("fake:0").unwrap();
    assert_eq!(caps.resolutions, [150, 300]);

    let mut bad = request();
    bad.settings.dpi = 1200;
    let err = svc.submit(bad).unwrap_err();
    assert!(matches!(
        err.downcast_ref::<ServiceError>(),
        Some(ServiceError::Invalid(_))
    ));

    let mut duplex = request();
    duplex.settings.source = Source::FeederDuplex;
    assert!(svc.submit(duplex).is_err());
}

#[test]
fn finished_scan_is_delivered_and_failures_are_reported() {
    let dir = TempDir::new("deliver");
    let good = Recorder::new("good", false);
    let bad = Recorder::new("bad", true);
    let svc = service_with(2, 0, &dir, vec![good.clone(), bad]);
    assert_eq!(svc.destinations().len(), 2);

    let mut req = request();
    req.destinations = vec!["good".into(), "bad".into()];
    let job = wait_finished(&svc, svc.submit(req).unwrap());

    assert_eq!(
        job.state,
        JobState::Done,
        "a failed upload doesn't fail the scan"
    );
    assert_eq!(*good.delivered.lock().unwrap(), job.files);
    assert_eq!(job.deliveries.len(), 2);
    assert!(job.deliveries[0].result.is_ok());
    assert_eq!(job.deliveries[1].result, Err("upload refused".into()));
    assert!(svc.file_path(&job.files[0]).is_some(), "local copy is kept");
}

#[test]
fn unknown_destination_is_rejected_on_submit() {
    let dir = TempDir::new("unknown_dest");
    let svc = service(1, 0, &dir);
    let mut req = request();
    req.destinations = vec!["nowhere".into()];
    let err = svc.submit(req).unwrap_err();
    assert_eq!(
        err.downcast_ref::<ServiceError>(),
        Some(&ServiceError::Invalid(
            "unknown destination 'nowhere'".into()
        ))
    );
}

#[test]
fn cancelled_scan_is_not_delivered() {
    let dir = TempDir::new("cancel_dest");
    let rec = Recorder::new("rec", false);
    let svc = service_with(50, 20, &dir, vec![rec.clone()]);
    let mut req = request();
    req.destinations = vec!["rec".into()];
    let id = svc.submit(req).unwrap();
    while svc.job(id).unwrap().pages == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    svc.cancel(id);
    let job = wait_finished(&svc, id);
    assert_eq!(job.state, JobState::Cancelled);
    assert!(job.deliveries.is_empty());
    assert!(rec.delivered.lock().unwrap().is_empty());
}

#[test]
fn local_copy_deleted_only_when_every_destination_succeeded() {
    let dir = TempDir::new("delete_ok");
    let svc = service_with(
        1,
        0,
        &dir,
        vec![Recorder::new("a", false), Recorder::new("b", false)],
    );
    let mut req = request();
    req.destinations = vec!["a".into(), "b".into()];
    req.delete_after_delivery = true;
    let job = wait_finished(&svc, svc.submit(req).unwrap());
    assert_eq!(job.deleted_files, job.files);
    assert!(svc.file_path(&job.files[0]).is_none(), "local copy removed");
}

#[test]
fn local_copy_kept_when_any_upload_failed() {
    let dir = TempDir::new("delete_partial");
    let svc = service_with(
        1,
        0,
        &dir,
        vec![Recorder::new("ok", false), Recorder::new("bad", true)],
    );
    let mut req = request();
    req.destinations = vec!["ok".into(), "bad".into()];
    req.delete_after_delivery = true;
    let job = wait_finished(&svc, svc.submit(req).unwrap());
    assert!(job.deleted_files.is_empty());
    assert!(svc.file_path(&job.files[0]).is_some(), "local copy kept");
}

#[test]
fn delete_without_destination_is_rejected() {
    let dir = TempDir::new("delete_nodest");
    let svc = service(1, 0, &dir);
    let mut req = request();
    req.delete_after_delivery = true;
    assert!(svc.submit(req).is_err());
}

#[test]
fn failed_upload_is_retried_until_it_succeeds_then_local_copy_deleted() {
    let dir = TempDir::new("retry");
    let flaky = Flaky::new(2);
    let svc = fast_retry_service(&dir, vec![flaky.clone()]);
    let mut req = request();
    req.destinations = vec!["flaky".into()];
    req.delete_after_delivery = true;
    let job = wait_finished(&svc, svc.submit(req).unwrap());

    // First attempt failed: local copy kept and queued for retry.
    assert_eq!(job.state, JobState::Done);
    assert!(job.deliveries[0].result.is_err());
    assert!(job.deleted_files.is_empty());

    wait_until("retries to succeed", || svc.pending_uploads().is_empty());
    assert_eq!(flaky.delivered.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        svc.file_path(&job.files[0]).is_none(),
        "deleted after the retry succeeded"
    );
    assert!(!dir.0.join(OUTBOX_FILE).exists());
}

#[test]
fn pending_uploads_survive_a_restart() {
    let dir = TempDir::new("restart");
    let file;
    {
        // Hourly retries: nothing is retried during this test.
        let svc = service_with(1, 0, &dir, vec![Flaky::new(usize::MAX)]);
        let mut req = request();
        req.destinations = vec!["flaky".into()];
        let job = wait_finished(&svc, svc.submit(req).unwrap());
        file = job.files[0].clone();
        let pending = svc.pending_uploads();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].file, file);
        assert_eq!(pending[0].last_error, "temporarily unavailable");
        assert!(pending[0].next_attempt >= pending[0].created + 3600);
    }
    assert!(dir.0.join(OUTBOX_FILE).exists());

    // "Restart" with a working destination: the leftover upload is sent at startup.
    let ok = Flaky::new(0);
    let svc = service_with(1, 0, &dir, vec![ok.clone()]);
    svc.retry_uploads_now().unwrap();
    wait_until("upload after restart", || svc.pending_uploads().is_empty());
    assert_eq!(ok.delivered.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(svc.file_path(&file).is_some(), "kept: delete-after was off");
}

#[test]
fn retry_now_and_attempt_counting() {
    let dir = TempDir::new("retry_now");
    let svc = service_with(1, 0, &dir, vec![Flaky::new(2)]);
    let mut req = request();
    req.destinations = vec!["flaky".into()];
    wait_finished(&svc, svc.submit(req).unwrap());
    assert_eq!(svc.pending_uploads()[0].attempts, 1);

    svc.retry_uploads_now().unwrap(); // fails again (2nd failure)
    wait_until("second attempt", || {
        svc.pending_uploads()
            .first()
            .is_some_and(|p| p.attempts == 2)
    });
    svc.retry_uploads_now().unwrap(); // succeeds
    wait_until("success", || svc.pending_uploads().is_empty());
    assert!(svc.pending_uploads_error().is_none());
}
