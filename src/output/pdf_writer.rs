//! Tiny PDF writer: one JPEG image per page, page size derived from the scan DPI.

use std::io::Write;

pub struct PdfPage {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub gray: bool,
    pub dpi: f64,
}

pub fn write_pdf(out: &mut impl Write, pages: &[PdfPage]) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();

    buf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    // Object numbering: 1 = catalog, 2 = page tree, then 3 objects per page.
    let page_obj = |i: usize| 3 + i * 3;
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", page_obj(i)))
        .collect();

    let obj = |buf: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    };

    obj(&mut buf, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(
        &mut buf,
        &mut offsets,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    );

    for (i, p) in pages.iter().enumerate() {
        let n = page_obj(i);
        let dpi = if p.dpi > 0.0 { p.dpi } else { 300.0 };
        let w = p.width as f64 * 72.0 / dpi;
        let h = p.height as f64 * 72.0 / dpi;

        obj(
            &mut buf,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w:.2} {h:.2}] \
                 /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                n + 2,
                n + 1
            )
            .as_bytes(),
        );

        let content = format!("q {w:.2} 0 0 {h:.2} 0 0 cm /Im0 Do Q");
        obj(
            &mut buf,
            &mut offsets,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .as_bytes(),
        );

        let mut img = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /{} \
             /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            p.width,
            p.height,
            if p.gray { "DeviceGray" } else { "DeviceRGB" },
            p.jpeg.len()
        )
        .into_bytes();
        img.extend_from_slice(&p.jpeg);
        img.extend_from_slice(b"\nendstream");
        obj(&mut buf, &mut offsets, &img);
    }

    let xref = buf.len();
    buf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for off in &offsets {
        buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );

    out.write_all(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xref_offsets_point_at_objects() {
        let pages = vec![
            PdfPage {
                jpeg: vec![0xFF, 0xD8, 0xFF, 0xD9],
                width: 600,
                height: 300,
                gray: false,
                dpi: 300.0,
            },
            PdfPage {
                jpeg: vec![0xFF, 0xD8, 0xFF, 0xD9],
                width: 300,
                height: 300,
                gray: true,
                dpi: 300.0,
            },
        ];
        let mut out = Vec::new();
        write_pdf(&mut out, &pages).unwrap();
        // Work on bytes: the binary header comment is not valid UTF-8.
        let find = |needle: &[u8]| out.windows(needle.len()).rposition(|w| w == needle);
        let has = |needle: &str| find(needle.as_bytes()).is_some();
        assert!(has("/Count 2"));
        assert!(has("/MediaBox [0 0 144.00 72.00]"));
        assert!(has("/DeviceGray"));

        let tail = &out[find(b"startxref\n").unwrap() + 10..];
        let startxref: usize = std::str::from_utf8(tail)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let xref = std::str::from_utf8(&out[startxref..]).unwrap();
        assert!(xref.starts_with("xref"));
        for (i, line) in xref.lines().skip(3).take(8).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(
                out[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()),
                "object {}",
                i + 1
            );
        }
    }
}
