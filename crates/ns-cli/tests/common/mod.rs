//! Shared helpers for the CLI integration tests, and for `ns-analyzer`'s
//! behaviour oracle of the hints, which includes this file by its path.
//!
//! Each test binary that needs these declares `mod common;`, so this file is
//! compiled once per binary rather than linked once -- which is why the
//! allow below is here: a binary that uses only some of the helpers would
//! otherwise warn about the rest.

#![allow(
    dead_code,
    reason = "a test binary that uses only some of the helpers would otherwise warn about the rest"
)]

use std::path::{Path, PathBuf};

#[path = "../../src/target_runner.rs"]
pub mod target_runner;

/// A command that starts `binary`, one of the workspace's executables, as
/// cargo starts a test binary (`target_runner.rs`).
pub fn target_command(binary: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    target_runner::target_command(binary)
}

/// The repository's root, two levels above this crate.
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `goldens/` directory: every recorded program outcome lives in a
/// category directory below it.
pub fn goldens_root() -> PathBuf {
    repository_root().join("goldens")
}

/// Every benchmark program with a recorded outcome, sorted: each micro
/// benchmark under `benchmarks/micro/`, and the Are We Fast Yet project,
/// which verifies every benchmark at its full size. Each has its `.toml`
/// beside it, as a golden has, and is held to it as a golden is -- but only
/// when benchmarking and at the end of a branch, since a benchmark is sized
/// to be measured, not to be run on every commit.
pub fn benchmark_programs() -> Vec<PathBuf> {
    let benchmarks = repository_root().join("benchmarks");
    let mut programs: Vec<PathBuf> = directory_entries(&benchmarks.join("micro"))
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "ns"))
        .collect();
    programs.push(benchmarks.join("are-we-fast-yet/newspeak"));
    programs.sort();
    for program in &programs {
        assert!(
            program.with_extension("toml").is_file(),
            "{} has no recorded outcome beside it",
            program.display()
        );
    }
    programs
}

/// One run of a benchmark program: the program, the arguments it is run
/// with, and the outcome it must give.
pub struct BenchmarkRun {
    /// `benchmarks/micro/bench_loop`, or for one benchmark of the Are We
    /// Fast Yet harness `benchmarks/are-we-fast-yet/newspeak/Havlak`.
    pub name: String,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub expectation: Expectation,
}

/// Every run the profile `benchmarks-verify` holds to its outcome: each
/// micro benchmark as its `.toml` records it, and the Are We Fast Yet
/// harness once per benchmark its `.toml` lists -- `<Name>: ok` for the
/// harness run with that name -- so that the benchmarks run in parallel
/// rather than one after another in a single process.
pub fn benchmark_runs() -> Vec<BenchmarkRun> {
    let mut runs = Vec::new();
    for program in benchmark_programs() {
        let expectation = expectation_of(&program);
        if !program.is_dir() {
            runs.push(BenchmarkRun {
                name: benchmark_name(&program),
                arguments: expectation.arguments.clone(),
                program,
                expectation,
            });
            continue;
        }
        for name in benchmark_names(&expectation) {
            runs.push(BenchmarkRun {
                name: format!("{}/{name}", benchmark_name(&program)),
                program: program.clone(),
                arguments: vec![name.clone()],
                expectation: Expectation {
                    status: 0,
                    stdout: format!("{name}: ok\n"),
                    stderr: String::new(),
                    skipped: None,
                    arguments: vec![name],
                    reaches_a_table_limit: None,
                    rules: Vec::new(),
                },
            });
        }
    }
    runs
}

/// The benchmarks the Are We Fast Yet harness's recorded outcome lists, one
/// `<Name>: ok` line each, in its order.
pub fn benchmark_names(expectation: &Expectation) -> Vec<String> {
    let names: Vec<String> = expectation
        .stdout
        .lines()
        .map(|line| {
            line.strip_suffix(": ok")
                .unwrap_or_else(|| panic!("`{line}` is no `<Name>: ok` line"))
                .to_string()
        })
        .collect();
    assert!(
        !names.is_empty(),
        "the harness's outcome lists no benchmark"
    );
    names
}

/// The `-- argument ...` a command that sends a program's entry is given:
/// nothing when there are no `arguments`.
pub fn after_the_separator(arguments: &[String]) -> Vec<std::ffi::OsString> {
    if arguments.is_empty() {
        return Vec::new();
    }
    std::iter::once("--".into())
        .chain(arguments.iter().map(Into::into))
        .collect()
}

/// A benchmark program's path below the repository root without its
/// extension, `/`-separated on every platform: `benchmarks/micro/bench_loop`.
pub fn benchmark_name(program: &Path) -> String {
    program
        .with_extension("")
        .strip_prefix(repository_root())
        .unwrap_or_else(|_| panic!("{} is not in the repository", program.display()))
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every golden file with `extension`, found by walking rather than from a
/// list, so that adding one needs no edit anywhere. Sorted, so a failure names
/// the same golden on every platform and run.
///
/// The goldens are a tree of directories: one that holds files is a category
/// and holds goldens only; one that holds none groups categories or further
/// groups. Anything else is refused rather than skipped -- a file at the root,
/// or a directory beside goldens. The one exception is a project golden: the
/// directory `<name>/` holding a `newspeak.json` and that project's modules,
/// its recorded outcome `<name>.toml` beside it as every golden's is. Its
/// contents are not goldens and are not walked (`golden_projects`). A golden the walk does not
/// accept but silently drops is not a golden at all: no sweep runs it and the
/// pairing check cannot see it, so a wrong recorded answer would pass
/// unnoticed. The category is the organizing principle, so a file outside one
/// is a mistake to report, not a layout to support.
pub fn golden_files(extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let root = goldens_root();
    for entry in directory_entries(&root) {
        assert!(
            entry.is_dir(),
            "{} is a file at the root of {} -- every golden belongs in a category directory",
            entry.display(),
            root.display()
        );
        walk_golden_directory(&entry, extension, &mut files);
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "no .{extension} files found under {} -- the directory moved or the walk is wrong",
        root.display()
    );
    files
}

/// Collects the goldens with `extension` in `directory`, a category if it
/// holds any file and a group of directories otherwise.
fn walk_golden_directory(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    let entries = directory_entries(directory);
    assert!(
        !entries.is_empty(),
        "{} is empty -- a directory below goldens/ holds goldens or further directories",
        directory.display()
    );
    let is_category = entries.iter().any(|entry| entry.is_file());
    for path in entries {
        if path.is_file() {
            if path.extension().and_then(|found| found.to_str()) == Some(extension) {
                files.push(path);
            }
        } else if !is_category {
            walk_golden_directory(&path, extension, files);
        } else {
            assert!(
                is_project_golden(&path),
                "{} is a directory inside the category {} with no {PROJECT_FILE} in it -- \
                 a category holds goldens, and only a project golden's directory may live \
                 one level deeper",
                path.display(),
                directory.display(),
            );
        }
    }
}

fn directory_entries(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| entry.expect("goldens directory entry").path())
        .collect()
}

/// Every golden's source: the `.ns` programs only.
pub fn golden_sources() -> Vec<PathBuf> {
    golden_files("ns")
}

/// The one name a project file has (`ns-cli`'s `project::PROJECT_FILE_NAME`).
pub const PROJECT_FILE: &str = "newspeak.json";

/// Whether `directory`, inside a category, is a project golden: it holds the
/// project's `newspeak.json`.
fn is_project_golden(directory: &Path) -> bool {
    directory.join(PROJECT_FILE).is_file()
}

/// Every project golden: the directory of each `newspeak.json` one level
/// below a category, which is what `ns-next` is given to run it, sorted. Its
/// recorded outcome is the directory's path with `.toml`, so a project
/// directory with no `.toml` beside it is found by the pairing check's other
/// direction rather than here.
pub fn golden_projects() -> Vec<PathBuf> {
    let categories: std::collections::BTreeSet<PathBuf> = golden_files("toml")
        .iter()
        .filter_map(|expectation| expectation.parent().map(Path::to_path_buf))
        .collect();
    let mut projects: Vec<PathBuf> = categories
        .iter()
        .flat_map(|category| directory_entries(category))
        .filter(|entry| entry.is_dir() && is_project_golden(entry))
        .collect();
    projects.sort();
    projects
}

/// Every golden program, a `.ns` source or a project golden's directory,
/// sorted.
pub fn golden_programs() -> Vec<PathBuf> {
    let mut programs = golden_files("ns");
    programs.extend(golden_projects());
    programs.sort();
    programs
}

/// One golden's source by its path below `goldens/`, without the extension --
/// `golden("library/number/integer/arithmetic")`. Naming the category is what
/// makes the answer unambiguous: two categories may hold the same name, and a
/// caller that says which one it means cannot be surprised by a golden added
/// later somewhere else. Panics on a miss, so a renamed or moved golden fails
/// loudly here rather than silently skipping the test that wanted it.
pub fn golden(path: &str) -> PathBuf {
    let parts: Vec<&str> = path.split('/').collect();
    assert!(
        parts.len() >= 2 && parts.iter().all(|part| !part.is_empty()),
        "{path} is not `<category>/<name>` -- a golden lives in a category \
         directory below goldens/, so its path has at least one directory component"
    );
    let (name, directories) = parts.split_last().expect("at least two parts");
    let mut source = goldens_root();
    source.extend(directories);
    source.push(format!("{name}.ns"));
    assert!(
        source.is_file(),
        "no golden at {} -- goldens/{path}.ns does not exist",
        source.display()
    );
    source
}

/// The golden's path below `goldens/` without its extension, `/`-separated on
/// every platform -- the name `golden` takes, so a failing trial names the
/// golden the same way a test that wants it does.
pub fn golden_name(source: &Path) -> String {
    let relative = source
        .with_extension("")
        .strip_prefix(goldens_root())
        .unwrap_or_else(|_| panic!("{} is not below goldens/", source.display()))
        .to_path_buf();
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
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
    /// The program's arguments, passed after `--` to every command that
    /// sends the entry: what `main:args:` receives (§8.4). Empty for most.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Why the program grows a table beside the object space past its
    /// growth limit's floor with rows it keeps live, so that a table's
    /// limit may request a collection (`heap::limits`); every other
    /// program is held to requesting none
    /// (`no_program_reaches_a_table_limit`).
    #[serde(default)]
    pub reaches_a_table_limit: Option<String>,
    /// The identifiers of the specification's rules the golden checks
    /// (`docs/spec/introduction.md`, "Conventions"), written on one line
    /// and read by `cargo xtask spec`; nothing reads them at run time.
    #[serde(default)]
    pub rules: Vec<String>,
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
/// No golden `.toml` is covered by `.gitattributes` and `core.autocrlf` is
/// true, so a fresh Windows checkout really does write a CRLF `.toml`, while
/// the CLI always writes `\n`. TOML's multi-line literal string is specified as taken
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
    if let Some(reason) = &recorded.reaches_a_table_limit {
        // As for `skipped`: an exemption says why it is one.
        assert!(
            !reason.trim().is_empty(),
            "{} has an empty `reaches_a_table_limit` -- a golden reaches a \
             table's limit only for a reason that says which rows it keeps live",
            path.display()
        );
    }
    if let Some(reason) = &recorded.skipped {
        // A blank reason is the whole failure mode the field could
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
        arguments: recorded.arguments,
        reaches_a_table_limit: recorded.reaches_a_table_limit,
        rules: recorded.rules,
    }
}

/// The `-- argument ...` a command that sends `source`'s entry is given: its
/// golden's recorded `arguments`, or nothing when it records none.
pub fn program_arguments(source: &Path) -> Vec<std::ffi::OsString> {
    let arguments = expectation_of(source).arguments;
    if arguments.is_empty() {
        return Vec::new();
    }
    std::iter::once("--".into())
        .chain(arguments.into_iter().map(Into::into))
        .collect()
}

/// Normalizes the line endings of what the process wrote. The recorded side
/// is already normalized by `expectation_of`, which explains why;
/// this is the same normalization applied to the other side, so that a
/// platform's own line endings can never be mistaken for a difference in
/// program behaviour.
pub fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// The one comparison every golden run makes, reporting which of the three parts
/// differs and how.
pub fn compare(
    output: &std::process::Output,
    expectation: &Expectation,
    condition: &str,
) -> Result<(), String> {
    if output.status.code() != Some(expectation.status) {
        return Err(format!(
            "exit status {condition}: expected {}, got {:?}
stderr:
{}",
            expectation.status,
            output.status.code(),
            normalize_newlines(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    let stdout = normalize_newlines(&String::from_utf8_lossy(&output.stdout));
    if stdout != expectation.stdout {
        return Err(format!(
            "stdout {condition}:
--- expected
{}--- got
{stdout}",
            expectation.stdout
        ));
    }
    let stderr = normalize_newlines(&String::from_utf8_lossy(&output.stderr));
    if stderr != expectation.stderr {
        return Err(format!(
            "stderr {condition}:
--- expected
{}--- got
{stderr}",
            expectation.stderr
        ));
    }
    Ok(())
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

/// `.temp/` at the repository root, two levels above the crate.
fn scratch_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate is two levels below the repository root")
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
/// Only `ns-cli`'s own tests have it from cargo; the module still compiles
/// where they do not.
pub fn ns_next(arguments: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let Some(binary) = option_env!("CARGO_BIN_EXE_ns-next") else {
        panic!("cargo hands the ns-next binary only to ns-cli's own tests");
    };
    let output = target_command(binary)
        .args(arguments)
        .output()
        .expect("run ns-next");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
    )
}

/// Removes `--collector-statistics`'s line from `output`'s stderr and
/// answers it; `None` when the run stopped before it printed one, as a
/// program that does not compile does.
///
/// The line is the last one that starts `collector: ` and ends with the
/// table collections: the run prints it after everything the program
/// wrote, so a line of the program's own that looks alike comes before
/// it, and a program's stderr that ends without a newline is fused with
/// it, which leaves the program's part in place.
pub fn take_collector_statistics(output: &mut std::process::Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let start = stderr
        .match_indices("collector: ")
        .map(|(start, _)| start)
        .filter(|start| {
            let line = stderr[*start..].lines().next().unwrap_or_default();
            line.ends_with(" table collections")
        })
        .last()?;
    let line_end = stderr[start..]
        .find('\n')
        .map_or(stderr.len(), |offset| start + offset + 1);
    let statistics = stderr[start..line_end].trim_end().to_string();
    output.stderr = format!("{}{}", &stderr[..start], &stderr[line_end..]).into_bytes();
    Some(statistics)
}

/// The collector's statistics a run under `expectation` printed, which
/// every program that reached its entry send must print
/// (`take_collector_statistics`) -- one that succeeds, or one whose entry
/// send ended in an uncaught error: a run that stopped printing them would
/// otherwise pass the table check unchecked. A program that fails before
/// its entry send, as one that does not compile does, prints none.
pub fn require_collector_statistics(
    statistics: Option<String>,
    expectation: &Expectation,
) -> Result<Option<String>, String> {
    let reached_the_entry_send =
        expectation.succeeds() || expectation.stderr.starts_with("Uncaught Newspeak error: ");
    match statistics {
        None if reached_the_entry_send => {
            Err("the run printed no `--collector-statistics` line".to_string())
        }
        statistics => Ok(statistics),
    }
}

/// No golden or benchmark program grows a table beside the object space
/// to its growth limit (`heap::limits`) unless its `expectation` says why
/// (`Expectation::reaches_a_table_limit`): the floors lie above what the
/// others mint, so that a small program never collects for a table.
pub fn no_program_reaches_a_table_limit(
    statistics: &str,
    expectation: &Expectation,
) -> Result<(), String> {
    if expectation.reaches_a_table_limit.is_some() || statistics.ends_with("; 0 table collections")
    {
        Ok(())
    } else {
        Err(format!(
            "a table's growth limit requested a collection: {statistics}"
        ))
    }
}
