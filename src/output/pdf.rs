//! Collects pages into a single PDF, written when the batch finishes.

use std::path::PathBuf;

use anyhow::Result;

use super::pdf_writer::{write_pdf, PdfPage};
use super::{encode_jpeg, write_atomically, PageSink};
use crate::scanner::Page;

pub struct PdfSink {
    path: PathBuf,
    quality: u8,
    pages: Vec<PdfPage>,
}

impl PdfSink {
    pub fn new(path: PathBuf, quality: u8) -> Self {
        Self {
            path,
            quality,
            pages: Vec::new(),
        }
    }
}

impl PageSink for PdfSink {
    fn add(&mut self, page: &Page) -> Result<()> {
        // Encode immediately so we hold compressed bytes, not raw bitmaps.
        let (jpeg, gray) = encode_jpeg(&page.image, self.quality)?;
        self.pages.push(PdfPage {
            jpeg,
            width: page.image.width(),
            height: page.image.height(),
            gray,
            dpi: page.dpi as f64,
        });
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<Vec<PathBuf>> {
        if self.pages.is_empty() {
            return Ok(Vec::new());
        }
        let mut bytes = Vec::new();
        write_pdf(&mut bytes, &self.pages)?;
        write_atomically(&self.path, &bytes)?;
        Ok(vec![self.path])
    }
}
