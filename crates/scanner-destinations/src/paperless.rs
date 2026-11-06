//! Paperless-ngx: uploads via `POST /api/documents/post_document/`.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use scanner_core::destination::{Destination, DestinationInfo};
use scanner_core::output::content_type;

use crate::config::{PaperlessConfig, Secret};
use crate::multipart::Multipart;

pub struct Paperless {
    id: String,
    label: String,
    endpoint: String,
    token: Secret,
    tags: Vec<u64>,
    agent: ureq::Agent,
}

impl Paperless {
    pub fn new(id: String, label: String, config: &PaperlessConfig) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            // Read error bodies ourselves so Paperless' message reaches the user.
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            id,
            label,
            endpoint: format!(
                "{}/api/documents/post_document/",
                config.url.trim_end_matches('/')
            ),
            token: config.token.clone(),
            tags: config.tags.clone(),
            agent,
        }
    }
}

impl Destination for Paperless {
    fn info(&self) -> DestinationInfo {
        DestinationInfo {
            id: self.id.clone(),
            label: self.label.clone(),
            kind: "paperless",
        }
    }

    fn deliver(&self, file: &Path) -> Result<String> {
        let data = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "scan".into());
        let title = file
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());

        let mut form = Multipart::new();
        form.text("title", &title);
        for tag in &self.tags {
            form.text("tags", &tag.to_string());
        }
        form.file("document", &name, content_type(file), &data);
        let (form_type, body) = form.finish();

        let mut response = self
            .agent
            .post(&self.endpoint)
            .header("Authorization", format!("Token {}", self.token.expose()))
            .header("Content-Type", form_type)
            .header("Accept", "application/json")
            .send(&body[..])
            .context("uploading to Paperless")?;
        let status = response.status();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        if !status.is_success() {
            let snippet: String = text.chars().take(200).collect();
            bail!("Paperless rejected the upload ({status}): {snippet}");
        }
        // Paperless returns the consumption task id as a JSON string.
        Ok(format!("Paperless task {}", text.trim().trim_matches('"')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// Serves one request, returns (request head, body) and answers with `reply`.
    fn one_shot_server(
        reply: &'static str,
    ) -> (String, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            stream.write_all(reply.as_bytes()).unwrap();
            (head, body)
        });
        (url, handle)
    }

    fn destination(url: String) -> Paperless {
        let cfg = PaperlessConfig {
            url,
            token: Secret::for_test("abc"),
            tags: vec![7],
        };
        Paperless::new("p".into(), "P".into(), &cfg)
    }

    fn pdf_file(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("paperless_{tag}_{}.pdf", std::process::id()));
        std::fs::write(&p, b"%PDF-1.4 test").unwrap();
        p
    }

    #[test]
    fn uploads_with_token_title_tags_and_file() {
        let (url, server) = one_shot_server(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\n\"task-123\"",
        );
        let file = pdf_file("ok");
        let result = destination(url).deliver(&file);
        let (head, body) = server.join().unwrap();
        std::fs::remove_file(&file).unwrap();

        assert_eq!(result.unwrap(), "Paperless task task-123");
        assert!(head.starts_with("POST /api/documents/post_document/ HTTP/1.1"));
        assert!(
            head.contains("authorization: Token abc") || head.contains("Authorization: Token abc")
        );
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("name=\"tags\"\r\n\r\n7\r\n"));
        assert!(body.contains("name=\"title\"\r\n\r\npaperless_ok_"));
        assert!(body.contains("Content-Type: application/pdf\r\n\r\n%PDF-1.4 test"));
    }

    #[test]
    fn reports_server_error_message() {
        let (url, server) = one_shot_server(
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 27\r\n\r\n{\"detail\":\"Invalid token.\"}",
        );
        let file = pdf_file("err");
        let err = destination(url).deliver(&file).unwrap_err();
        server.join().unwrap();
        std::fs::remove_file(&file).unwrap();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("401") && msg.contains("Invalid token"),
            "{msg}"
        );
    }
}
