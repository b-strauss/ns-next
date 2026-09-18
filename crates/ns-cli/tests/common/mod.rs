//! Shared helpers for the CLI integration tests.
//!
//! Each test binary that needs these declares `mod common;`, so this file is
//! compiled once per binary rather than linked once -- which is why the
//! allow below is here: a binary that uses only some of the helpers would
//! otherwise warn about the rest.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The `goldens/` directory: every recorded program outcome lives under a
/// category directory inside it.
pub fn goldens_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../goldens")
}

/// Every golden file with `extension`, found by walking rather than from a
/// list, so that adding one needs no edit anywhere. Sorted, so a failure names
/// the same golden on every platform and run.
///
/// The layout is exactly two levels -- `goldens/<category>/<name>.<extension>`
/// -- and anything else is refused rather than skipped. A golden the walk does
/// not accept but silently drops is not a golden at all: no sweep runs it and
/// the pairing check cannot see it, so a wrong recorded answer would pass
/// unnoticed. The category is the organizing principle, so a file outside one
/// is a mistake to report, not a layout to support.
pub fn golden_files(extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let root = goldens_root();
    let categories =
        std::fs::read_dir(&root).unwrap_or_else(|error| panic!("read {}: {error}", root.display()));
    for category in categories {
        let category = category.expect("goldens category entry").path();
        assert!(
            category.is_dir(),
            "{} is a file at the root of {} -- every golden belongs in a category directory",
            category.display(),
            root.display()
        );
        let entries = std::fs::read_dir(&category)
            .unwrap_or_else(|error| panic!("read {}: {error}", category.display()));
        for entry in entries {
            let path = entry.expect("golden entry").path();
            assert!(
                !path.is_dir(),
                "{} is a directory inside the category {} -- a golden lives one level \
                 below {}, never deeper",
                path.display(),
                category.display(),
                root.display()
            );
            if path.extension().and_then(|found| found.to_str()) == Some(extension) {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "no .{extension} files found under {} -- the directory moved or the walk is wrong",
        root.display()
    );
    files
}

/// Every golden's source.
pub fn golden_sources() -> Vec<PathBuf> {
    golden_files("ns")
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
    /// Whether the golden is expected to exit zero -- the recorded answer to
    /// "does this program run to completion", which a sweep asserts against
    /// rather than assuming.
    pub fn succeeds(&self) -> bool {
        self.status == 0
    }
}

/// The recorded outcome beside `source`, with its line endings normalized.
///
/// `.gitattributes` covers only `corpus/**` and `core.autocrlf` is true, so a
/// fresh Windows checkout really does write a CRLF `.toml`, while the CLI
/// always writes `\n`. TOML's multi-line literal string is specified as taken
/// as-is, so nothing in the format promises the `\r` is dropped -- today's
/// parser drops it, but that is its choice, not the format's. Normalizing here
/// makes every reader of a recorded answer right by construction instead of by
/// a dependency's behaviour.
pub fn expectation_of(source: &Path) -> Expectation {
    let path = source.with_extension("toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let recorded: Expectation =
        toml::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    Expectation {
        status: recorded.status,
        stdout: recorded.stdout.replace("\r\n", "\n"),
        stderr: recorded.stderr.replace("\r\n", "\n"),
    }
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
/// tests naming the same golden would otherwise share a directory and delete
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
