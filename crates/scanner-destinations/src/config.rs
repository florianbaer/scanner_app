//! TOML configuration for destinations.
//!
//! ```toml
//! [[destination]]
//! id = "paperless"
//! type = "paperless"
//! url = "https://paperless.example.com"
//! token = "env:PAPERLESS_TOKEN"
//! ```
//!
//! Any secret may be written as `env:VARIABLE` to read it from the environment
//! instead of storing it in the file.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, rename = "destination")]
    pub destinations: Vec<DestinationConfig>,
}

#[derive(Debug, Deserialize)]
pub struct DestinationConfig {
    /// Stable id used to pick this destination (UI, `--to`). `[a-z0-9_-]`.
    pub id: String,
    /// Display name; defaults to the id.
    pub label: Option<String>,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Kind {
    Paperless(PaperlessConfig),
    S3(S3Config),
}

#[derive(Debug, Deserialize)]
pub struct PaperlessConfig {
    /// Base URL, e.g. `https://paperless.example.com`.
    pub url: String,
    /// API token (Paperless: Profile → API Auth Token).
    pub token: Secret,
    /// Tags for every uploaded document: ids (`7`) or names (`"scanning-app"`).
    /// Names are looked up (and created if missing) via the API, which needs the
    /// Paperless user to have "Tags: View" (and "Tags: Add" to create).
    #[serde(default)]
    pub tags: Vec<TagRef>,
}

/// A Paperless tag, by id or by name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum TagRef {
    Id(u64),
    Name(String),
}

#[derive(Debug, Deserialize)]
pub struct S3Config {
    /// Endpoint URL, e.g. `https://fsn1.your-objectstorage.com`.
    pub endpoint: String,
    /// Region name the endpoint expects, e.g. `fsn1` or `eu-central-1`.
    pub region: String,
    pub bucket: String,
    /// Key prefix, e.g. `scans/`.
    #[serde(default)]
    pub prefix: String,
    pub access_key: Secret,
    pub secret_key: Secret,
    /// Use `endpoint/bucket/key` URLs (MinIO); default is `bucket.endpoint/key`.
    #[serde(default)]
    pub path_style: bool,
}

/// A secret value: literal, or `env:NAME` resolved at load time. Never printed.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Secret(value.into())
    }

    fn resolve(raw: String) -> Result<Self, String> {
        match raw.strip_prefix("env:") {
            Some(var) => std::env::var(var)
                .map(Secret)
                .map_err(|_| format!("environment variable {var} is not set")),
            None => Ok(Secret(raw)),
        }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Secret::resolve(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for d in &self.destinations {
            let valid = !d.id.is_empty()
                && d.id.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-')
                });
            if !valid {
                bail!("destination id '{}' must be non-empty [a-z0-9_-]", d.id);
            }
            if !seen.insert(&d.id) {
                bail!("destination id '{}' is used twice", d.id);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_kinds_and_env_secrets() {
        std::env::set_var("SCANNER_TEST_TOKEN", "t0ken");
        let c = Config::parse(
            r#"
            [[destination]]
            id = "paperless"
            type = "paperless"
            url = "https://p.example"
            token = "env:SCANNER_TEST_TOKEN"
            tags = [3, "scanning-app"]

            [[destination]]
            id = "archive"
            label = "Archive"
            type = "s3"
            endpoint = "https://s3.example"
            region = "fsn1"
            bucket = "scans"
            prefix = "inbox/"
            access_key = "AK"
            secret_key = "SK"
            "#,
        )
        .unwrap();
        assert_eq!(c.destinations.len(), 2);
        let Kind::Paperless(p) = &c.destinations[0].kind else {
            panic!()
        };
        assert_eq!(p.token.expose(), "t0ken");
        assert_eq!(p.tags, [TagRef::Id(3), TagRef::Name("scanning-app".into())]);
        let Kind::S3(s) = &c.destinations[1].kind else {
            panic!()
        };
        assert_eq!(
            (s.bucket.as_str(), s.prefix.as_str(), s.path_style),
            ("scans", "inbox/", false)
        );
        assert!(
            !format!("{c:?}").contains("t0ken"),
            "secrets must not leak via Debug"
        );
    }

    #[test]
    fn rejects_missing_env_bad_and_duplicate_ids() {
        let missing = r#"
            [[destination]]
            id = "p"
            type = "paperless"
            url = "u"
            token = "env:SCANNER_TEST_DEFINITELY_UNSET"
        "#;
        assert!(format!("{:#}", Config::parse(missing).unwrap_err()).contains("is not set"));

        let bad_id = missing
            .replace("id = \"p\"", "id = \"Bad Id\"")
            .replace("env:SCANNER_TEST_DEFINITELY_UNSET", "x");
        assert!(Config::parse(&bad_id).is_err());

        let one = r#"
            [[destination]]
            id = "p"
            type = "paperless"
            url = "u"
            token = "x"
        "#;
        assert!(Config::parse(&format!("{one}{one}")).is_err());
        assert!(Config::parse("").unwrap().destinations.is_empty());
    }
}
