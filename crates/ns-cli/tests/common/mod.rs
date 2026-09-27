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

/// One golden's source by its path below `goldens/`, without the extension --
/// `golden("language/arithmetic")`. Naming the category is what makes the
/// answer unambiguous: two categories may hold the same name, and a caller
/// that says which one it means cannot be surprised by a golden added later
/// somewhere else. Panics on a miss, so a renamed or moved golden fails
/// loudly here rather than silently skipping the test that wanted it.
pub fn golden(path: &str) -> PathBuf {
    let mut parts = path.split('/');
    let category = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    assert!(
        !category.is_empty() && !name.is_empty() && parts.next().is_none(),
        "{path} is not `<category>/<name>` -- a golden lives one level below \
         goldens/, so its path has exactly one directory component"
    );
    let source = goldens_root().join(category).join(format!("{name}.ns"));
    assert!(
        source.is_file(),
        "no golden at {} -- goldens/{path}.ns does not exist",
        source.display()
    );
    source
}

/// `<category>/<name>`, the golden's path below `goldens/` without its
/// extension -- the name `golden` takes, so a failing trial names the
/// golden the same way a test that wants it does.
pub fn golden_name(source: &Path) -> String {
    let category = source
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let name = source
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    format!("{category}/{name}")
}

/// A golden's recorded outcome. Every field defaults to the success case, so
/// an ordinary golden's file is just its `stdout` and a failing one's is its
/// `status` and `stderr`. `deny_unknown_fields` turns a misspelled key into a
/// failure instead of an expectation that silently stops being checked.
///
/// `skipped` carries why the runtime does not answer this yet. The other
/// three fields still record what it *should* answer, so a golden written
/// for a known gap states the whole expectation up front, and closing the
/// gap is deleting one line rather than rediscovering months later what
/// the answer was meant to be.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub skipped: Option<String>,
}

impl Expectation {
    /// Whether the golden is expected to exit zero -- the recorded answer to
    /// "does this program run to completion", which a sweep asserts against
    /// rather than assuming.
    pub fn succeeds(&self) -> bool {
        self.status == 0
    }

    /// Why this golden is not run, if it is not. A sweep skips it and
    /// counts it; the pairing check still holds it to being a complete
    /// pair, so `skipped` cannot smuggle a half-written golden past
    /// discovery.
    pub fn skipped(&self) -> Option<&str> {
        self.skipped.as_deref()
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
    if let Some(reason) = &recorded.skipped {
        // A blank reason is the whole failure mode this field could
        // introduce: a golden switched off in a hurry, with nothing left
        // saying what would switch it back on.
        assert!(
            !reason.trim().is_empty(),
            "{} has an empty `skipped` -- a golden is skipped only with a \
             reason that says what would let it run",
            path.display()
        );
    }
    Expectation {
        status: recorded.status,
        stdout: recorded.stdout.replace("\r\n", "\n"),
        stderr: recorded.stderr.replace("\r\n", "\n"),
        skipped: recorded.skipped,
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

/// One source, snapshotted and reloaded: everything `run` produces --
/// stdout, stderr and exit status -- is exactly what the initializer
/// produces while snapshotting plus what the entry send produces after a
/// load. Returns the three `run` outputs so a caller can assert what the
/// program itself did.
pub fn assert_snapshot_load_matches_run(name: &str, source: &Path) -> (bool, String, String) {
    let directory = ScratchDirectory::new(name);
    let image = directory.join("world.img");
    let (run_ok, run_out, run_error) = ns_next(&["run".as_ref(), source.as_os_str()]);
    let (snapshot_ok, snapshot_out, snapshot_error) =
        ns_next(&["snapshot".as_ref(), source.as_os_str(), image.as_os_str()]);
    let (load_ok, load_out, load_error) = if snapshot_ok {
        ns_next(&[
            "load".as_ref(),
            image.as_os_str(),
            "--send".as_ref(),
            "main:args:".as_ref(),
        ])
    } else {
        // `snapshot` sends no entry, so it fails only where `run` already
        // failed before its own entry send -- while instantiating the root
        // class. There is then no image and nothing further to compare: its
        // own streams carry the whole outcome.
        (snapshot_ok, String::new(), String::new())
    };
    // `snapshot`'s own last line ("wrote <path>") is the command's report,
    // not the program's output; everything before it is what the module
    // printed while being instantiated.
    let printed_while_snapshotting = snapshot_out
        .strip_suffix(&format!("wrote {}\n", image.display()))
        .unwrap_or(&snapshot_out)
        .to_string();
    assert_eq!(
        format!("{printed_while_snapshotting}{load_out}"),
        run_out,
        "{name}: snapshot-then-load must print exactly what `run` prints"
    );
    // stderr too: `report_entry_send` is shared, so an `Uncaught Newspeak
    // error:` line must come out identical on both paths -- comparing only
    // stdout would let the two diverge on exactly the messages the goldens'
    // recorded `stderr` pins byte for byte.
    assert_eq!(
        format!("{snapshot_error}{load_error}"),
        run_error,
        "{name}: snapshot-then-load must report exactly what `run` reports"
    );
    assert_eq!(
        snapshot_ok && load_ok,
        run_ok,
        "{name}: snapshot-then-load must succeed exactly when `run` does"
    );
    (run_ok, run_out, run_error)
}
