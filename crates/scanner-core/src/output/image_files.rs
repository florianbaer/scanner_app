//! Writes each page as its own JPEG or PNG file.

use std::io::Cursor;
use std::path::PathBuf;

use anyhow::Result;
use image::ImageFormat;

use super::{encode_jpeg, page_file_name, write_atomically, Format, OutputSpec, PageSink};
use crate::scanner::Page;

pub struct ImageSink {
    spec: OutputSpec,
    written: Vec<PathBuf>,
}

impl ImageSink {
    pub fn new(spec: OutputSpec) -> Self {
        Self {
            spec,
            written: Vec::new(),
        }
    }
}

impl PageSink for ImageSink {
    fn add(&mut self, page: &Page) -> Result<()> {
        let (bytes, ext) = match self.spec.format {
            Format::Png => {
                let mut buf = Cursor::new(Vec::new());
                page.image.write_to(&mut buf, ImageFormat::Png)?;
                (buf.into_inner(), "png")
            }
            Format::Jpeg | Format::Pdf => {
                (encode_jpeg(&page.image, self.spec.jpeg_quality)?.0, "jpg")
            }
        };
        let path = self
            .spec
            .dir
            .join(page_file_name(&self.spec.stem, page.sheet, page.side, ext));
        write_atomically(&path, &bytes)?;
        self.written.push(path);
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<Vec<PathBuf>> {
        Ok(self.written)
    }
}
