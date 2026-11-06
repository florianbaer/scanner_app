//! Service tests against a fake backend: no hardware needed.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use image::{DynamicImage, RgbImage};

use super::*;
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
    let backend = FakeBackend {
        pages,
        page_delay: Duration::from_millis(delay_ms),
    };
    ScannerService::start(backend, dir.0.clone()).unwrap()
}

fn request() -> ScanRequest {
    ScanRequest {
        device: "fake:0".into(),
        settings: ScanSettings::default(),
        format: Format::Pdf,
        jpeg_quality: 80,
        name: "test".into(),
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
