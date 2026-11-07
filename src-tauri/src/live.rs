//! Single-publisher lease: an exclusive file lock that decides which process publishes Discord
//! presence.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;

use anyhow::{Context, Result};
use fs2::FileExt;

pub struct PublisherLease {
    path: PathBuf,
    file: Option<File>,
}

impl PublisherLease {
    pub fn new(path: PathBuf) -> Self {
        Self { path, file: None }
    }

    pub fn try_acquire(&mut self) -> Result<bool> {
        if self.file.is_some() {
            return Ok(true);
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create publisher lock directory {}",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.path)
            .with_context(|| format!("failed to open publisher lock {}", self.path.display()))?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                self.file = Some(file);
                Ok(true)
            }
            Err(error) if lock_is_held(&error) => Ok(false),
            Err(error) => Err(error).with_context(|| {
                format!("failed to acquire publisher lock {}", self.path.display())
            }),
        }
    }

    pub fn release(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
    }
}

fn lock_is_held(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock || error.raw_os_error() == Some(33)
}

#[cfg(test)]
mod tests {
    use super::PublisherLease;

    #[test]
    fn only_one_publisher_owns_a_runtime_lock() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("presence.lock");
        let mut first = PublisherLease::new(path.clone());
        let mut second = PublisherLease::new(path.clone());

        assert!(first.try_acquire().expect("first lease"));
        assert!(!second.try_acquire().expect("second lease"));

        drop(first);

        assert!(second.try_acquire().expect("released lease"));
    }
}
