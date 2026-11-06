//! The folder of finished scans: listing files and resolving download names safely.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

/// A finished scan file in the output directory.
#[derive(Debug, Clone)]
pub struct StoredFile {
    pub name: String,
    pub size: u64,
    pub modified: SystemTime,
}

const EXTENSIONS: [&str; 4] = ["pdf", "jpg", "jpeg", "png"];

/// Lists scan files, newest first. A missing directory is an empty library.
pub fn list_files(dir: &Path) -> Result<Vec<StoredFile>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_safe_name(&name) || !has_scan_extension(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() {
            files.push(StoredFile {
                name,
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    files.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.name.cmp(&a.name)));
    Ok(files)
}

/// Resolves a user-supplied file name to a path inside `dir`, or `None` if the
/// name could escape the directory or isn't a scan file.
pub fn resolve(dir: &Path, name: &str) -> Option<PathBuf> {
    if !is_safe_name(name) || !has_scan_extension(name) {
        return None;
    }
    let path = dir.join(name);
    path.is_file().then_some(path)
}

/// Only plain names made of `[A-Za-z0-9._-]`, not starting with a dot.
fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn has_scan_extension(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_names() {
        let dir = std::env::temp_dir();
        for bad in [
            "../x.pdf",
            "a/b.pdf",
            ".hidden.pdf",
            "x.txt",
            "",
            "x y.pdf",
            "x\".pdf",
        ] {
            assert!(resolve(&dir, bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn lists_only_scan_files_newest_first() {
        let dir = std::env::temp_dir().join(format!("scanner_app_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.pdf"), b"1").unwrap();
        fs::write(dir.join("notes.txt"), b"1").unwrap();
        fs::write(dir.join("b.jpg"), b"12").unwrap();
        let names: Vec<_> = list_files(&dir)
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"a.pdf".to_string()) && names.contains(&"b.jpg".to_string()));
        assert!(list_files(&dir).unwrap().is_empty());
    }
}
