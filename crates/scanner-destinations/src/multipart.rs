//! Minimal `multipart/form-data` body builder (ureq's multipart API is unstable).

pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        Self::with_boundary(format!("scanner-app-{nanos:x}"))
    }

    fn with_boundary(boundary: String) -> Self {
        Self {
            boundary,
            body: Vec::new(),
        }
    }

    pub fn text(&mut self, name: &str, value: &str) -> &mut Self {
        self.part_header(name, None, None);
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    pub fn file(
        &mut self,
        name: &str,
        filename: &str,
        content_type: &str,
        data: &[u8],
    ) -> &mut Self {
        self.part_header(name, Some(filename), Some(content_type));
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// Returns the `Content-Type` header value and the body.
    pub fn finish(mut self) -> (String, Vec<u8>) {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (
            format!("multipart/form-data; boundary={}", self.boundary),
            self.body,
        )
    }

    fn part_header(&mut self, name: &str, filename: Option<&str>, content_type: Option<&str>) {
        let mut h = format!(
            "--{}\r\nContent-Disposition: form-data; name=\"{}\"",
            self.boundary,
            escape(name)
        );
        if let Some(f) = filename {
            h.push_str(&format!("; filename=\"{}\"", escape(f)));
        }
        h.push_str("\r\n");
        if let Some(ct) = content_type {
            h.push_str(&format!("Content-Type: {ct}\r\n"));
        }
        h.push_str("\r\n");
        self.body.extend_from_slice(h.as_bytes());
    }
}

fn escape(s: &str) -> String {
    s.replace(['"', '\r', '\n'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_rfc7578_body() {
        let mut m = Multipart::with_boundary("B".into());
        m.text("title", "Scan 1")
            .file("document", "a\".pdf", "application/pdf", b"%PDF");
        let (ct, body) = m.finish();
        assert_eq!(ct, "multipart/form-data; boundary=B");
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "--B\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nScan 1\r\n\
             --B\r\nContent-Disposition: form-data; name=\"document\"; filename=\"a_.pdf\"\r\n\
             Content-Type: application/pdf\r\n\r\n%PDF\r\n--B--\r\n"
        );
    }
}
