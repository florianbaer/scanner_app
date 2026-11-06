//! Converts raw scanner line data into images. Pure and backend-independent.

use anyhow::{bail, Result};
use image::{DynamicImage, GrayImage, RgbImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channels {
    Gray,
    Rgb,
}

#[derive(Debug, Clone, Copy)]
pub struct RasterFormat {
    pub channels: Channels,
    /// Bits per sample: 1, 8 or 16 (16-bit samples are in native byte order).
    pub depth: u32,
    /// Bytes per line, including any padding.
    pub bytes_per_line: usize,
    /// Pixels per line.
    pub width: u32,
}

/// Decodes `data` into an 8-bit image. Trailing partial lines are ignored.
pub fn decode(f: &RasterFormat, data: &[u8]) -> Result<DynamicImage> {
    if f.bytes_per_line == 0 || f.width == 0 {
        bail!("scanner reported an empty image format");
    }
    let height = (data.len() / f.bytes_per_line) as u32;
    if height == 0 {
        bail!("scanner returned no image data");
    }
    let w = f.width as usize;
    let samples = match f.channels {
        Channels::Gray => 1,
        Channels::Rgb => 3,
    };
    let needed = match f.depth {
        1 if f.channels == Channels::Rgb => bail!("1-bit color scans are not supported"),
        1 => w.div_ceil(8),
        8 => w * samples,
        16 => w * samples * 2,
        d => bail!("unsupported bit depth {d}"),
    };
    if needed > f.bytes_per_line {
        bail!(
            "line too short: {} bytes for {} pixels at depth {}",
            f.bytes_per_line,
            w,
            f.depth
        );
    }

    let mut out = Vec::with_capacity(w * samples * height as usize);
    for line in data.chunks_exact(f.bytes_per_line) {
        match f.depth {
            // SANE lineart: bit set = black, MSB first.
            1 => {
                for x in 0..w * samples {
                    let bit = line[x / 8] & (0x80 >> (x % 8));
                    out.push(if bit != 0 { 0 } else { 255 });
                }
            }
            8 => out.extend_from_slice(&line[..w * samples]),
            16 => out.extend(
                line[..w * samples * 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| (u16::from_ne_bytes(*b) >> 8) as u8),
            ),
            _ => unreachable!(),
        }
    }

    Ok(match f.channels {
        Channels::Gray => DynamicImage::ImageLuma8(
            GrayImage::from_raw(f.width, height, out).expect("size checked"),
        ),
        Channels::Rgb => {
            DynamicImage::ImageRgb8(RgbImage::from_raw(f.width, height, out).expect("size checked"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray8_strips_line_padding() {
        let f = RasterFormat {
            channels: Channels::Gray,
            depth: 8,
            bytes_per_line: 4,
            width: 3,
        };
        let img = decode(&f, &[1, 2, 3, 99, 4, 5, 6, 99])
            .unwrap()
            .into_luma8();
        assert_eq!(img.dimensions(), (3, 2));
        assert_eq!(img.as_raw(), &[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn rgb8_and_partial_trailing_line_ignored() {
        let f = RasterFormat {
            channels: Channels::Rgb,
            depth: 8,
            bytes_per_line: 6,
            width: 2,
        };
        let img = decode(&f, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap().into_rgb8();
        assert_eq!(img.dimensions(), (2, 1));
        assert_eq!(img.get_pixel(1, 0).0, [4, 5, 6]);
    }

    #[test]
    fn lineart_set_bit_is_black() {
        let f = RasterFormat {
            channels: Channels::Gray,
            depth: 1,
            bytes_per_line: 1,
            width: 3,
        };
        let img = decode(&f, &[0b1010_0000]).unwrap().into_luma8();
        assert_eq!(img.as_raw(), &[0, 255, 0]);
    }

    #[test]
    fn gray16_takes_high_byte() {
        let f = RasterFormat {
            channels: Channels::Gray,
            depth: 16,
            bytes_per_line: 2,
            width: 1,
        };
        let img = decode(&f, &0xAB12u16.to_ne_bytes()).unwrap().into_luma8();
        assert_eq!(img.as_raw(), &[0xAB]);
    }

    #[test]
    fn empty_data_is_an_error() {
        let f = RasterFormat {
            channels: Channels::Gray,
            depth: 8,
            bytes_per_line: 4,
            width: 4,
        };
        assert!(decode(&f, &[]).is_err());
    }
}
