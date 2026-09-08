//! Shared helpers for the CLI integration tests.
//!
//! Each test binary that needs these declares `mod common;`, so this file is
//! compiled once per binary rather than linked once -- which is why the
//! allow below is here: a binary that uses only two of the three helpers
//! would otherwise warn about the third.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A file under `tests-fixtures/`, by name.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests-fixtures")
        .join(name)
}

/// A temporary directory that removes itself.
///
/// The earlier helpers cleared a stale directory on the way *in* and left the
/// fresh one behind on the way out, so every run of the suite deposited one
/// directory per test in the system temp folder and nothing ever collected
/// them. Clearing on entry bounded the mess -- the same test reused the same
/// path -- but "bounded" is not "cleaned up", and a developer's temp folder is
/// not the suite's to litter.
///
/// The name carries the process id and a counter as well as the test's own
/// name because cargo runs a binary's tests as threads of one process: two
/// tests naming the same fixture would otherwise share a directory and delete
/// each other's files.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub fn new(test: &str) -> ScratchDir {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ns-next-test-{}-{n}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        ScratchDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the directory. The file need not exist yet.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best effort on purpose. A failing test unwinds through here, and a
        // panic during a panic aborts the process -- which would replace the
        // assertion message that explains the failure with one that does not.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Runs the `ns-next` binary and returns `(success, stdout, stderr)`, with
/// line endings normalized so an assertion reads the same on every platform.
pub fn ns_next(args: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .args(args)
        .output()
        .expect("run ns-next");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
    )
}
