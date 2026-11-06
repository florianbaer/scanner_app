//! Where scanned pages go: a `PageSink` per output format.

mod image_files;
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
    fn page_names() {
        assert_eq!(page_file_name("s", 1, Side::Single, "jpg"), "s_001.jpg");
        assert_eq!(
            page_file_name("s", 1, Side::Front, "jpg"),
            "s_001_front.jpg"
        );
        assert_eq!(page_file_name("s", 12, Side::Back, "png"), "s_012_back.png");
    }
}
