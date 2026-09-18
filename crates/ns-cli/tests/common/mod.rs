//! Shared helpers for the CLI integration tests.
//!
//! Each test binary that needs these declares `mod common;`, so this file is
//! compiled once per binary rather than linked once -- which is why the
//! allow below is here: a binary that uses only two of the three helpers
//! would otherwise warn about the third.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The `goldens/` directory: every recorded program outcome lives under a
/// category directory inside it.
pub fn goldens_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../goldens")
}

/// Every golden's source, found by walking rather than from a list, so that
/// adding one needs no edit anywhere. Sorted, so a failure names the same
/// golden on every platform and run.
pub fn golden_sources() -> Vec<PathBuf> {
    let mut sources = Vec::new();
    let root = goldens_root();
    let categories =
        std::fs::read_dir(&root).unwrap_or_else(|error| panic!("read {}: {error}", root.display()));
    for category in categories {
        let category = category.expect("goldens category entry").path();
        if !category.is_dir() {
            continue;
        }
        let entries = std::fs::read_dir(&category)
            .unwrap_or_else(|error| panic!("read {}: {error}", category.display()));
        for entry in entries {
            let path = entry.expect("golden entry").path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("ns") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    assert!(
        !sources.is_empty(),
        "no goldens found under {} -- the directory moved or the walk is wrong",
        root.display()
    );
    sources
}

/// One golden's source by its bare name, for the few tests that need a
/// particular program rather than all of them. Panics on a miss so that a
/// renamed golden fails loudly here instead of silently skipping a test, and
/// on an ambiguity so that two categories cannot quietly claim one name.
pub fn golden(name: &str) -> PathBuf {
    let matches: Vec<PathBuf> = golden_sources()
        .into_iter()
        .filter(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(name))
        .collect();
    match matches.len() {
        1 => matches.into_iter().next().expect("one match"),
        0 => panic!("no golden named {name}"),
        _ => panic!("{} goldens named {name}", matches.len()),
    }
}

/// A golden's recorded outcome. Every field defaults to the success case, so
/// an ordinary golden's file is just its `stdout` and a failing one's is its
/// `status` and `stderr`. `deny_unknown_fields` turns a misspelled key into a
/// failure instead of an expectation that silently stops being checked.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

impl Expectation {
    /// Whether the golden is expected to exit zero. The snapshot and
    /// build/load round trips only apply to a program that runs to
    /// completion, so they use this to skip the rest.
    pub fn succeeds(&self) -> bool {
        self.status == 0
    }
}

/// The recorded outcome beside `source`.
pub fn expectation_of(source: &Path) -> Expectation {
    let path = source.with_extension("toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    toml::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// A temporary directory that removes itself, under the repository's own
/// `.temp/` (git-ignored) rather than the system temporary folder.
///
/// A developer's temporary folder is not the suite's to litter, and helpers that
/// forgot to clean up have filled it twice before -- once with a directory
/// per test, once with one per run. Keeping scratch space inside the
/// repository means that whatever a killed or aborted run still leaves
/// behind stays where it is visible and belongs to the project, and deleting
/// `.temp/` clears it all.
///
/// The name carries the process id and a counter as well as the test's own
/// name because cargo runs a binary's tests as threads of one process: two
/// tests naming the same fixture would otherwise share a directory and delete
/// each other's files.
pub struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    pub fn new(test: &str) -> ScratchDirectory {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = scratch_root().join(format!("{}-{sequence}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        ScratchDirectory { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the directory. The file need not exist yet.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

/// `.temp/` at the repository root, two levels above this crate.
fn scratch_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/ns-cli is two levels below the repository root")
        .join(".temp")
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        // Best effort on purpose. A failing test unwinds through here, and a
        // panic during a panic aborts the process -- which would replace the
        // assertion message that explains the failure with one that does not.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Runs the `ns-next` binary and returns `(success, stdout, stderr)`, with
/// line endings normalized so an assertion reads the same on every platform.
pub fn ns_next(arguments: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .args(arguments)
        .output()
        .expect("run ns-next");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
    )
}
