//! `ns-next-analyzer fmt`, run as the binary over temporary directories
//! under the system's own, never over the repository's files.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

const UNFORMATTED: &str = "class Sample = Object new ( ) ( public run = ( 1+2 ) ) : ( )\n";
const SYNTAX_ERROR: &str = "class Broken = Object new ( ( public\n";

/// A directory that removes itself. The name carries the process id and a
/// counter as well as the test's name because cargo runs a binary's tests as
/// threads of one process.
struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(test: &str) -> TemporaryDirectory {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ns-next-fmt-{}-{sequence}-{test}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temporary directory");
        TemporaryDirectory { path }
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path.join(name);
        std::fs::write(&path, text).expect("write test file");
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        // Best effort: a failing test unwinds through here.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn run(directory: &Path, arguments: &[&Path], check: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ns-next-analyzer"));
    command.current_dir(directory).arg("fmt");
    if check {
        command.arg("--check");
    }
    command.args(arguments).output().expect("run fmt")
}

fn formatted(source: &str) -> String {
    ns_syntax::format(source).expect("the test source is valid")
}

#[test]
fn fmt_rewrites_an_unformatted_file_and_leaves_a_formatted_one_untouched() {
    let directory = TemporaryDirectory::new("rewrite");
    let unformatted = directory.write("Unformatted.ns", UNFORMATTED);
    let already = directory.write("Already.ns", &formatted(UNFORMATTED));
    // Pushed back so that a rewrite, however quick, would show.
    let past = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&already)
        .unwrap()
        .set_modified(past)
        .unwrap();
    let before = std::fs::metadata(&already).unwrap().modified().unwrap();

    let output = run(&directory.path, &[&unformatted, &already], false);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&unformatted).unwrap(),
        formatted(UNFORMATTED)
    );
    assert_ne!(std::fs::read_to_string(&unformatted).unwrap(), UNFORMATTED);
    assert_eq!(
        std::fs::metadata(&already).unwrap().modified().unwrap(),
        before
    );
}

#[test]
fn check_prints_the_path_of_a_file_that_would_change_and_writes_nothing() {
    let directory = TemporaryDirectory::new("check-change");
    let unformatted = directory.write("Unformatted.ns", UNFORMATTED);

    let output = run(&directory.path, &[&unformatted], true);

    assert_eq!(output.status.code(), Some(1));
    let printed = String::from_utf8_lossy(&output.stdout);
    assert_eq!(printed.trim_end(), unformatted.display().to_string());
    assert_eq!(std::fs::read_to_string(&unformatted).unwrap(), UNFORMATTED);
}

#[test]
fn check_on_formatted_files_succeeds_silently() {
    let directory = TemporaryDirectory::new("check-clean");
    directory.write("Already.ns", &formatted(UNFORMATTED));

    let output = run(&directory.path, &[&directory.path], true);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
}

#[test]
fn a_syntax_error_is_reported_and_the_other_files_are_still_formatted() {
    let directory = TemporaryDirectory::new("syntax-error");
    let broken = directory.write("Broken.ns", SYNTAX_ERROR);
    let valid = directory.write("Valid.ns", UNFORMATTED);

    let output = run(&directory.path, &[&directory.path], false);

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Broken.ns"));
    assert_eq!(std::fs::read_to_string(&broken).unwrap(), SYNTAX_ERROR);
    assert_eq!(
        std::fs::read_to_string(&valid).unwrap(),
        formatted(UNFORMATTED)
    );
}

#[test]
fn without_a_path_the_current_directory_is_formatted() {
    let directory = TemporaryDirectory::new("current-directory");
    let nested = directory.path.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let file = nested.join("Sample.ns");
    std::fs::write(&file, UNFORMATTED).unwrap();

    let output = run(&directory.path, &[], false);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        formatted(UNFORMATTED)
    );
}

#[test]
fn a_file_with_crlf_line_endings_stays_crlf() {
    let directory = TemporaryDirectory::new("crlf");
    let crlf_source = "class Sample = Object new ( )\r\n( public run = ( 1+2 ) ) : ( )\r\n";
    let file = directory.write("Sample.ns", crlf_source);

    let output = run(&directory.path, &[&file], false);

    assert_eq!(output.status.code(), Some(0));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\r\n"));
    assert!(!text.replace("\r\n", "").contains('\n'));
}

/// A byte-order mark is kept: the text after it is formatted, and the mark
/// written back in front of it.
#[test]
fn a_file_with_a_byte_order_mark_is_formatted_and_keeps_it() {
    let directory = TemporaryDirectory::new("byte-order-mark");
    let file = directory.write("Sample.ns", &format!("\u{feff}{UNFORMATTED}"));

    let output = run(&directory.path, &[&file], false);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("\u{feff}{}", formatted(UNFORMATTED))
    );
    let check = run(&directory.path, &[&file], true);
    assert_eq!(check.status.code(), Some(0), "{check:?}");
}
