//! Filesystem helpers shared by the release commands.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

pub fn to_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

/// Streams the file so multi-hundred-megabyte installers are never held in memory.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("open {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).with_context(|| format!("hash {}", path.display()))?;
    Ok(to_hex(&hasher.finalize()))
}

pub fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

pub fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Absolute form of `path` without resolving symlinks, so messages keep the caller's spelling.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("resolve {}", path.display()))
}

/// Every regular file below `root`, in no particular order.
pub fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries =
            fs::read_dir(&directory).with_context(|| format!("list {}", directory.display()))?;
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    Ok(files)
}

/// Files directly inside `directory`, sorted by byte-wise name.
pub fn list_files_sorted(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).with_context(|| format!("list {}", directory.display()))? {
        let path = entry?.path();
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort_by_key(|path| file_name(path));
    Ok(files)
}

/// Writes `SHA256SUMS.txt` (`<hash>  <name>` lines) covering every file already in `directory`.
/// Returns the number of lines.
pub fn write_sha256sums(directory: &Path) -> Result<usize> {
    let mut text = String::new();
    let files = list_files_sorted(directory)?;
    for path in &files {
        writeln!(text, "{}  {}", sha256_file(path)?, file_name(path))
            .expect("writing to a String cannot fail");
    }
    write_text(&directory.join("SHA256SUMS.txt"), &text)?;
    Ok(files.len())
}

/// A name part that is unique per process and call, for scratch folders.
pub fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!(
        "{:x}{:x}{:x}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Removes a scratch folder on drop, whether the command succeeded or not.
pub struct RemoveOnDrop(pub PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        // Best effort: the folder may already have been moved into place.
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_published_empty_and_abc_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn unique_suffix_never_repeats_within_a_process() {
        assert_ne!(unique_suffix(), unique_suffix());
    }
}
