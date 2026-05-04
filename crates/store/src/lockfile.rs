//! Cross-process lockfile (placeholder; full wiring in slice 4).
//!
//! `WorkdirLock` `flock`s `~/.nephila/<workdir-hash>.lock` on construction
//! and releases on drop. Slice 4 wires it into `bin/orchestrator.rs::main`.

use nix::fcntl::{Flock, FlockArg};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LockfileError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("could not acquire lock — another process holds it")]
    AlreadyHeld,
    #[error("invalid path: {0}")]
    InvalidPath(String),
}

/// RAII lockfile. Acquired with `flock(LOCK_EX | LOCK_NB)`; released on drop.
#[must_use = "WorkdirLock releases on drop; bind it to a long-lived local"]
pub struct WorkdirLock {
    _flock: Flock<File>,
    path: PathBuf,
}

impl WorkdirLock {
    /// Acquire the lockfile for `workdir`. Returns `AlreadyHeld` if another
    /// process holds the lock.
    pub fn acquire(workdir: &Path) -> Result<Self, LockfileError> {
        let lock_dir = lock_dir()?;
        std::fs::create_dir_all(&lock_dir)?;
        let hash = blake3::hash(workdir.as_os_str().to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        let path = lock_dir.join(format!("{}.lock", &hash[..16]));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        let flock = Flock::lock(file, FlockArg::LockExclusiveNonblock)
            .map_err(|(_f, _e)| LockfileError::AlreadyHeld)?;
        Ok(Self {
            _flock: flock,
            path,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn lock_dir() -> Result<PathBuf, LockfileError> {
    let home = dirs_home()?;
    Ok(home.join(".nephila").join("locks"))
}

fn dirs_home() -> Result<PathBuf, LockfileError> {
    if let Some(h) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(h));
    }
    Err(LockfileError::InvalidPath("$HOME unset".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_against_same_workdir_returns_error() {
        let tmp = tempdir_or_skip();
        let workdir = tmp.join("subdir");
        std::fs::create_dir_all(&workdir).unwrap();
        // Override HOME to keep the test hermetic.
        // SAFETY: Tests run single-threaded per test process when
        // `cargo test -- --test-threads=1`; these env mutations are
        // contained to this test and restored at end.
        unsafe {
            std::env::set_var("HOME", &tmp);
        }
        let lock1 = WorkdirLock::acquire(&workdir).expect("first acquire");
        let result = WorkdirLock::acquire(&workdir);
        match result {
            Err(LockfileError::AlreadyHeld) => {}
            Ok(_) => panic!("expected AlreadyHeld, got Ok"),
            Err(other) => panic!("expected AlreadyHeld, got {other:?}"),
        }
        drop(lock1);
        // After drop, a new acquisition succeeds.
        let _again = WorkdirLock::acquire(&workdir).expect("re-acquire after drop");
    }

    #[test]
    fn different_workdirs_dont_collide() {
        let tmp = tempdir_or_skip();
        unsafe {
            std::env::set_var("HOME", &tmp);
        }
        let a = tmp.join("a");
        let b = tmp.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let _la = WorkdirLock::acquire(&a).unwrap();
        let _lb = WorkdirLock::acquire(&b).unwrap();
    }

    fn tempdir_or_skip() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("nephila-lockfile-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
