//! S3-compatible object storage (Hetzner Object Storage, MinIO, AWS, …).

use std::path::Path;

use anyhow::{Context, Result};
use s3::creds::Credentials;
use s3::{Bucket, Region};
use scanner_core::destination::{Destination, DestinationInfo};
use scanner_core::output::content_type;

use crate::config::S3Config;

pub struct S3 {
    id: String,
    label: String,
    bucket: Box<Bucket>,
    prefix: String,
}

impl S3 {
    pub fn new(id: String, label: String, config: &S3Config) -> Result<Self> {
        let region = Region::Custom {
            region: config.region.clone(),
            endpoint: config.endpoint.clone(),
        };
        let credentials = Credentials::new(
            Some(config.access_key.expose()),
            Some(config.secret_key.expose()),
            None,
            None,
            None,
        )
        .context("S3 credentials")?;
        let mut bucket = Bucket::new(&config.bucket, region, credentials)
            .with_context(|| format!("configuring S3 bucket {}", config.bucket))?;
        if config.path_style {
            bucket = bucket.with_path_style();
        }
        Ok(Self {
            id,
            label,
            bucket,
            prefix: config.prefix.clone(),
        })
    }
}

/// Object key for a file: prefix + file name (prefix used verbatim, e.g. `scans/`).
fn object_key(prefix: &str, file: &Path) -> String {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{prefix}{name}")
}

impl Destination for S3 {
    fn info(&self) -> DestinationInfo {
        DestinationInfo {
            id: self.id.clone(),
            label: self.label.clone(),
            kind: "s3",
        }
    }

    fn deliver(&self, file: &Path) -> Result<String> {
        let data = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        let key = object_key(&self.prefix, file);
        // `fail-on-err` turns non-2xx responses into errors.
        self.bucket
            .put_object_with_content_type(&key, &data, content_type(file))
            .with_context(|| format!("uploading {key}"))?;
        Ok(format!("s3://{}/{key}", self.bucket.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_use_prefix_verbatim() {
        let f = Path::new("/tmp/scans/scan_1.pdf");
        assert_eq!(object_key("inbox/", f), "inbox/scan_1.pdf");
        assert_eq!(object_key("", f), "scan_1.pdf");
    }
}
