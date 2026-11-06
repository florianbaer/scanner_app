//! Where scanned pages go: a `PageSink` per output format.

mod image_files;
mod library;
mod pdf;
mod pdf_writer;

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType};

use crate::scanner::{Page, Side};

pub use image_files::ImageSink;
pub use library::{list_files, resolve, StoredFile};
pub use pdf::PdfSink;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One multi-page PDF per batch.
    Pdf,
    /// One JPEG per page.
    Jpeg,
    /// One PNG per page.
    Png,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Jpeg => "jpeg",
            Format::Png => "png",
        }
    }
}

impl std::str::FromStr for Format {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "pdf" => Ok(Format::Pdf),
            "jpeg" | "jpg" => Ok(Format::Jpeg),
            "png" => Ok(Format::Png),
            _ => Err(format!("unknown output format '{s}'")),
        }
    }
}

/// Receives pages during a scan and produces output files.
pub trait PageSink {
    fn add(&mut self, page: &Page) -> Result<()>;
    /// Finalises output and returns the files written.
    fn finish(self: Box<Self>) -> Result<Vec<PathBuf>>;
}

/// Output location and naming for one batch.
#[derive(Debug, Clone)]
pub struct OutputSpec {
    pub dir: PathBuf,
    /// File name stem, e.g. `scan_20261106_154500`.
    pub stem: String,
    pub format: Format,
    /// JPEG quality 1–100 (also used for images embedded in PDFs).
    pub jpeg_quality: u8,
}

impl OutputSpec {
    /// A spec named `<prefix>_<YYYYmmdd_HHMMSS>`, made unique within `dir`.
    /// The prefix is sanitised, so user input is safe to pass.
    pub fn timestamped(dir: PathBuf, prefix: &str, format: Format, jpeg_quality: u8) -> Self {
        let base = format!(
            "{}_{}",
            sanitize_prefix(prefix),
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        let stem = unique_stem(&dir, &base);
        Self {
            dir,
            stem,
            format,
            jpeg_quality,
        }
    }
}

/// Keeps `[A-Za-z0-9_-]`, max 40 chars; falls back to `scan`.
pub fn sanitize_prefix(prefix: &str) -> String {
    let s: String = prefix
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "scan".into()
    } else {
        s
    }
}

/// Appends `-2`, `-3`, … if any file in `dir` already starts with `base`.
fn unique_stem(dir: &Path, base: &str) -> String {
    let taken = |stem: &str| {
        fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(stem))
        })
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|s| !taken(s))
        .expect("unbounded range")
}

pub fn create_sink(spec: &OutputSpec) -> Result<Box<dyn PageSink>> {
    fs::create_dir_all(&spec.dir).with_context(|| format!("creating {}", spec.dir.display()))?;
    Ok(match spec.format {
        Format::Pdf => Box::new(PdfSink::new(
            spec.dir.join(format!("{}.pdf", spec.stem)),
            spec.jpeg_quality,
        )),
        Format::Jpeg | Format::Png => Box::new(ImageSink::new(spec.clone())),
    })
}

/// File name for a single page image, e.g. `scan_003_back.jpg`.
pub(crate) fn page_file_name(stem: &str, sheet: usize, side: Side, ext: &str) -> String {
    let suffix = match side {
        Side::Single => "",
        Side::Front => "_front",
        Side::Back => "_back",
    };
    format!("{stem}_{sheet:03}{suffix}.{ext}")
}

/// Encodes an image as baseline JPEG. Returns the bytes and whether it is grayscale.
pub(crate) fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<(Vec<u8>, bool)> {
    let mut buf = Cursor::new(Vec::new());
    let mut enc = JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
    let gray = matches!(img, DynamicImage::ImageLuma8(_));
    if gray {
        let g = img.to_luma8();
        enc.encode(g.as_raw(), g.width(), g.height(), ExtendedColorType::L8)?;
    } else {
        let rgb = img.to_rgb8();
        enc.encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )?;
    }
    Ok((buf.into_inner(), gray))
}

/// Writes via a temporary file and renames, so readers never see a partial file.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("part");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_is_sanitised() {
        assert_eq!(sanitize_prefix("Tax 2026/../x"), "Tax_2026____x");
        assert_eq!(sanitize_prefix("  "), "scan");
        assert_eq!(sanitize_prefix("ok-name_1"), "ok-name_1");
    }

    #[test]
    fn page_names() {
        assert_eq!(page_file_name("s", 1, Side::Single, "jpg"), "s_001.jpg");
        assert_eq!(
            page_file_name("s", 1, Side::Front, "jpg"),
            "s_001_front.jpg"
        );
        assert_eq!(page_file_name("s", 12, Side::Back, "png"), "s_012_back.png");
    }
}
