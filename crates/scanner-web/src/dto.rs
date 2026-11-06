//! JSON wire format. Kept separate from domain types so the API can stay stable
//! while the core evolves.

use std::time::SystemTime;

use scanner_core::destination::{Delivery, DestinationInfo};
use scanner_core::output::{Format, StoredFile};
use scanner_core::scanner::{Capabilities, ColorMode, FoundDevice, ScanSettings, Source};
use scanner_core::service::{DeviceList, Job, PendingUpload, ScanRequest};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;

fn timestamp(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t).to_rfc3339()
}

#[derive(Serialize)]
pub struct Device {
    pub name: String,
    pub label: String,
}

impl From<FoundDevice> for Device {
    fn from(d: FoundDevice) -> Self {
        Self {
            name: d.name,
            label: d.label,
        }
    }
}

#[derive(Serialize)]
pub struct Devices {
    pub devices: Vec<Device>,
    /// True if a scan was running and this is the last known list.
    pub cached: bool,
}

impl From<DeviceList> for Devices {
    fn from(l: DeviceList) -> Self {
        Self {
            devices: l.devices.into_iter().map(Into::into).collect(),
            cached: l.cached,
        }
    }
}

#[derive(Serialize)]
pub struct CapabilitiesDto {
    pub resolutions: Vec<u32>,
    pub modes: Vec<&'static str>,
    pub duplex: bool,
    pub auto_crop: bool,
    pub deskew: bool,
}

impl From<Capabilities> for CapabilitiesDto {
    fn from(c: Capabilities) -> Self {
        Self {
            resolutions: c.resolutions,
            modes: c.modes.into_iter().map(ColorMode::as_str).collect(),
            duplex: c.duplex,
            auto_crop: c.auto_crop,
            deskew: c.deskew,
        }
    }
}

#[derive(Serialize)]
pub struct DestinationDto {
    pub id: String,
    pub label: String,
    pub kind: &'static str,
}

impl From<DestinationInfo> for DestinationDto {
    fn from(d: DestinationInfo) -> Self {
        Self {
            id: d.id,
            label: d.label,
            kind: d.kind,
        }
    }
}

#[derive(Serialize)]
pub struct DeliveryDto {
    pub destination: String,
    pub file: String,
    pub ok: bool,
    /// Where it went, or the error message.
    pub message: String,
}

impl From<Delivery> for DeliveryDto {
    fn from(d: Delivery) -> Self {
        let (ok, message) = match d.result {
            Ok(m) => (true, m),
            Err(m) => (false, m),
        };
        Self {
            destination: d.destination,
            file: d.file,
            ok,
            message,
        }
    }
}

#[derive(Serialize)]
pub struct PendingUploadDto {
    pub destination: String,
    pub file: String,
    pub attempts: u32,
    pub last_error: String,
    pub next_attempt: String,
    pub delete_after_upload: bool,
}

impl From<PendingUpload> for PendingUploadDto {
    fn from(p: PendingUpload) -> Self {
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(p.next_attempt);
        Self {
            destination: p.destination,
            file: p.file,
            attempts: p.attempts,
            last_error: p.last_error,
            next_attempt: timestamp(at),
            delete_after_upload: p.delete_after,
        }
    }
}

#[derive(Serialize)]
pub struct PendingUploads {
    pub items: Vec<PendingUploadDto>,
    /// Problem saving the retry list, if any.
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct DeviceQuery {
    pub device: String,
}

#[derive(Deserialize)]
pub struct NewScan {
    pub device: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_resolution")]
    pub resolution: u32,
    #[serde(default)]
    pub duplex: bool,
    #[serde(default = "yes")]
    pub auto_crop: bool,
    #[serde(default = "yes")]
    pub deskew: bool,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_quality")]
    pub quality: u8,
    #[serde(default = "default_name")]
    pub name: String,
    /// Destination ids to also send the result to.
    #[serde(default)]
    pub destinations: Vec<String>,
    /// Remove the local copy once every destination accepted it.
    #[serde(default)]
    pub delete_after_upload: bool,
}

fn default_mode() -> String {
    "color".into()
}
fn default_resolution() -> u32 {
    300
}
fn yes() -> bool {
    true
}
fn default_format() -> String {
    "pdf".into()
}
fn default_quality() -> u8 {
    90
}
fn default_name() -> String {
    "scan".into()
}

impl TryFrom<NewScan> for ScanRequest {
    type Error = ApiError;

    fn try_from(n: NewScan) -> Result<Self, ApiError> {
        let mode: ColorMode = n.mode.parse().map_err(ApiError::bad_request)?;
        let format: Format = n.format.parse().map_err(ApiError::bad_request)?;
        Ok(ScanRequest {
            device: n.device,
            settings: ScanSettings {
                mode,
                source: if n.duplex {
                    Source::FeederDuplex
                } else {
                    Source::Feeder
                },
                dpi: n.resolution,
                auto_crop: n.auto_crop,
                deskew: n.deskew,
                extra_options: Vec::new(),
            },
            format,
            jpeg_quality: n.quality,
            name: n.name,
            destinations: n.destinations,
            delete_after_delivery: n.delete_after_upload,
        })
    }
}

#[derive(Serialize)]
pub struct Created {
    pub id: u64,
}

#[derive(Serialize)]
pub struct JobDto {
    pub id: u64,
    pub state: &'static str,
    pub device: String,
    pub mode: &'static str,
    pub resolution: u32,
    pub duplex: bool,
    pub format: &'static str,
    pub pages: usize,
    pub files: Vec<String>,
    pub destinations: Vec<String>,
    pub deliveries: Vec<DeliveryDto>,
    pub delete_after_upload: bool,
    pub deleted_files: Vec<String>,
    pub error: Option<String>,
    pub cancel_requested: bool,
    pub created: String,
    pub finished: Option<String>,
}

impl From<Job> for JobDto {
    fn from(j: Job) -> Self {
        let s = &j.request.settings;
        Self {
            id: j.id,
            state: j.state.as_str(),
            device: j.request.device.clone(),
            mode: s.mode.as_str(),
            resolution: s.dpi,
            duplex: s.source == Source::FeederDuplex,
            format: j.request.format.as_str(),
            pages: j.pages,
            files: j.files,
            destinations: j.request.destinations.clone(),
            deliveries: j.deliveries.into_iter().map(Into::into).collect(),
            delete_after_upload: j.request.delete_after_delivery,
            deleted_files: j.deleted_files,
            error: j.error,
            cancel_requested: j.cancel_requested,
            created: timestamp(j.created),
            finished: j.finished.map(timestamp),
        }
    }
}

#[derive(Serialize)]
pub struct FileDto {
    pub name: String,
    pub size: u64,
    pub modified: String,
    pub url: String,
}

impl From<StoredFile> for FileDto {
    fn from(f: StoredFile) -> Self {
        Self {
            // Names are restricted to [A-Za-z0-9._-] by core, so no URL escaping needed.
            url: format!("/files/{}", f.name),
            name: f.name,
            size: f.size,
            modified: timestamp(f.modified),
        }
    }
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    /// Show in the browser instead of downloading.
    #[serde(default)]
    pub inline: bool,
}
