//! Paperless-ngx: uploads via `POST /api/documents/post_document/`.
//! Tag names from the config are resolved to ids via `/api/tags/` (created if
//! missing) on first use and cached.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use scanner_core::destination::{Destination, DestinationInfo};
use scanner_core::output::content_type;

use crate::config::{PaperlessConfig, Secret, TagRef};
use crate::multipart::Multipart;

pub struct Paperless {
    id: String,
    label: String,
    /// Base URL without trailing slash, e.g. `https://paperless.example.com`.
    base: String,
    token: Secret,
    tags: Vec<TagRef>,
    /// Tag name (lowercase) → id, filled on first upload.
    tag_ids: Mutex<HashMap<String, u64>>,
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
            base: config.url.trim_end_matches('/').to_string(),
            token: config.token.clone(),
            tags: config.tags.clone(),
            tag_ids: Mutex::new(HashMap::new()),
            agent,
        }
    }

    fn auth(&self) -> String {
        format!("Token {}", self.token.expose())
    }

    /// Ids for all configured tags, resolving names through the API.
    fn resolve_tags(&self) -> Result<Vec<u64>> {
        self.tags
            .iter()
            .map(|t| match t {
                TagRef::Id(id) => Ok(*id),
                TagRef::Name(name) => self.tag_id(name),
            })
            .collect()
    }

    fn tag_id(&self, name: &str) -> Result<u64> {
        let key = name.to_lowercase();
        if let Some(id) = self
            .tag_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(*id);
        }
        let id = match self.find_tag(name)? {
            Some(id) => id,
            None => self.create_tag(name)?,
        };
        self.tag_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, id);
        Ok(id)
    }

    fn find_tag(&self, name: &str) -> Result<Option<u64>> {
        let mut response = self
            .agent
            .get(format!("{}/api/tags/", self.base))
            .query("name__iexact", name)
            .header("Authorization", self.auth())
            .header("Accept", "application/json")
            .call()
            .context("looking up Paperless tags")?;
        let status = response.status();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        if status.as_u16() == 403 {
            bail!(
                "the Paperless user may not view tags, so the tag name '{name}' can't be resolved. \
                 Grant it \"Tags: View\" in Paperless, or use the tag's numeric id in the config"
            );
        }
        if !status.is_success() {
            bail!("Paperless tag lookup failed ({status}): {}", snippet(&text));
        }
        let page: TagPage = serde_json::from_str(&text).context("reading Paperless tag list")?;
        Ok(page
            .results
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .map(|t| t.id))
    }

    fn create_tag(&self, name: &str) -> Result<u64> {
        // matching_algorithm 0 = "None": don't let Paperless auto-assign this tag
        // to unrelated documents.
        let body = serde_json::json!({ "name": name, "matching_algorithm": 0 }).to_string();
        let mut response = self
            .agent
            .post(format!("{}/api/tags/", self.base))
            .header("Authorization", self.auth())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(body.as_bytes())
            .context("creating Paperless tag")?;
        let status = response.status();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        if status.as_u16() == 403 {
            bail!(
                "tag '{name}' doesn't exist in Paperless and the Paperless user may not create tags. \
                 Create it in Paperless, or grant \"Tags: Add\""
            );
        }
        if !status.is_success() {
            bail!(
                "creating Paperless tag '{name}' failed ({status}): {}",
                snippet(&text)
            );
        }
        let tag: Tag = serde_json::from_str(&text).context("reading created Paperless tag")?;
        Ok(tag.id)
    }
}

#[derive(serde::Deserialize)]
struct TagPage {
    results: Vec<Tag>,
}

#[derive(serde::Deserialize)]
struct Tag {
    id: u64,
    name: String,
}

fn snippet(text: &str) -> String {
    text.chars().take(200).collect()
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

        let tags = self.resolve_tags()?;
        let mut form = Multipart::new();
        form.text("title", &title);
        for tag in tags {
            form.text("tags", &tag.to_string());
        }
        form.file("document", &name, content_type(file), &data);
        let (form_type, body) = form.finish();

        let mut response = self
            .agent
            .post(format!("{}/api/documents/post_document/", self.base))
            .header("Authorization", self.auth())
            .header("Content-Type", form_type)
            .header("Accept", "application/json")
            .send(&body[..])
            .context("uploading to Paperless")?;
        let status = response.status();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        if !status.is_success() {
            bail!(
                "Paperless rejected the upload ({status}): {}",
                snippet(&text)
            );
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

    type Request = (String, Vec<u8>);

    /// Answers one request per reply, in order (one connection each), and
    /// returns what it received as (request head, body).
    fn mock_server(
        replies: Vec<(u16, &'static str)>,
    ) -> (String, std::thread::JoinHandle<Vec<Request>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, body) in replies {
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
                let mut req_body = vec![0; len];
                reader.read_exact(&mut req_body).unwrap();
                let mut stream = stream;
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                seen.push((head, req_body));
            }
            seen
        });
        (url, handle)
    }

    fn destination(url: String, tags: Vec<TagRef>) -> Paperless {
        let cfg = PaperlessConfig {
            url,
            token: Secret::for_test("abc"),
            tags,
        };
        Paperless::new("p".into(), "P".into(), &cfg)
    }

    fn pdf_file(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("paperless_{tag}_{}.pdf", std::process::id()));
        std::fs::write(&p, b"%PDF-1.4 test").unwrap();
        p
    }

    fn body_str(r: &Request) -> String {
        String::from_utf8_lossy(&r.1).into_owned()
    }

    #[test]
    fn uploads_with_token_title_tags_and_file() {
        let (url, server) = mock_server(vec![(200, "\"task-123\"")]);
        let file = pdf_file("ok");
        let result = destination(url, vec![TagRef::Id(7)]).deliver(&file);
        let reqs = server.join().unwrap();
        std::fs::remove_file(&file).unwrap();

        assert_eq!(result.unwrap(), "Paperless task task-123");
        let (head, _) = &reqs[0];
        assert!(head.starts_with("POST /api/documents/post_document/ HTTP/1.1"));
        assert!(head
            .to_ascii_lowercase()
            .contains("authorization: token abc"));
        let body = body_str(&reqs[0]);
        assert!(body.contains("name=\"tags\"\r\n\r\n7\r\n"));
        assert!(body.contains("name=\"title\"\r\n\r\npaperless_ok_"));
        assert!(body.contains("Content-Type: application/pdf\r\n\r\n%PDF-1.4 test"));
    }

    #[test]
    fn existing_tag_name_is_looked_up_once_and_cached() {
        let (url, server) = mock_server(vec![
            (
                200,
                r#"{"count":1,"results":[{"id":5,"name":"Scanning-App"}]}"#,
            ),
            (200, "\"t1\""),
            (200, "\"t2\""),
        ]);
        let dest = destination(url, vec![TagRef::Name("scanning-app".into())]);
        let file = pdf_file("lookup");
        dest.deliver(&file).unwrap();
        dest.deliver(&file).unwrap(); // second upload: no lookup
        let reqs = server.join().unwrap();
        std::fs::remove_file(&file).unwrap();

        assert_eq!(reqs.len(), 3);
        assert!(reqs[0]
            .0
            .starts_with("GET /api/tags/?name__iexact=scanning-app "));
        for upload in &reqs[1..] {
            assert!(upload.0.starts_with("POST /api/documents/post_document/"));
            assert!(body_str(upload).contains("name=\"tags\"\r\n\r\n5\r\n"));
        }
    }

    #[test]
    fn missing_tag_is_created_without_auto_matching() {
        let (url, server) = mock_server(vec![
            (200, r#"{"count":0,"results":[]}"#),
            (201, r#"{"id":9,"name":"scanning-app"}"#),
            (200, "\"t\""),
        ]);
        let file = pdf_file("create");
        destination(url, vec![TagRef::Name("scanning-app".into())])
            .deliver(&file)
            .unwrap();
        let reqs = server.join().unwrap();
        std::fs::remove_file(&file).unwrap();

        assert!(reqs[1].0.starts_with("POST /api/tags/ "));
        let created: serde_json::Value = serde_json::from_slice(&reqs[1].1).unwrap();
        assert_eq!(created["name"], "scanning-app");
        assert_eq!(created["matching_algorithm"], 0);
        assert!(body_str(&reqs[2]).contains("name=\"tags\"\r\n\r\n9\r\n"));
    }

    #[test]
    fn missing_view_permission_explains_the_fix_and_skips_upload() {
        let (url, server) = mock_server(vec![(403, r#"{"detail":"no"}"#)]);
        let file = pdf_file("forbidden");
        let err = destination(url, vec![TagRef::Name("scanning-app".into())])
            .deliver(&file)
            .unwrap_err();
        let reqs = server.join().unwrap();
        std::fs::remove_file(&file).unwrap();
        assert_eq!(reqs.len(), 1, "no upload without the tag");
        assert!(format!("{err:#}").contains("Tags: View"));
    }

    #[test]
    fn reports_server_error_message() {
        let (url, server) = mock_server(vec![(401, r#"{"detail":"Invalid token."}"#)]);
        let file = pdf_file("err");
        let err = destination(url, vec![]).deliver(&file).unwrap_err();
        server.join().unwrap();
        std::fs::remove_file(&file).unwrap();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("401") && msg.contains("Invalid token"),
            "{msg}"
        );
    }
}
