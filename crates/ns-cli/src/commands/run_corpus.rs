//! `ns-next run-corpus`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::program::selector_of;
use ns_tooling::source::collect_ns_files;

/// One row of `corpus/runnable.toml`: a corpus module that is expected to
/// run, with everything needed to check that it still does.
///
/// `send` is the unary selector the generated wrapper application sends to
/// the module's instance. It is per entry because the corpus modules that run
/// today have no `main:args:` -- they are library
/// modules whose reachable entry point is a public method or a public
/// nested class's implicit accessor. A gate that assumed one selector
/// could not list any of them.
#[derive(serde::Deserialize)]
struct RunnableEntry {
    file: String,
    send: String,
    expected_status: i32,
    expected_stdout: String,
    /// Why this module is on the list. Required: the list is meant to grow
    /// one deliberate row at a time.
    #[expect(
        dead_code,
        reason = "documentation for the list's reader: it is parsed but never read by the runner"
    )]
    reason: String,
}

fn load_whitelist(path: &Path) -> Result<Vec<RunnableEntry>, String> {
    #[derive(serde::Deserialize)]
    struct WhitelistFile {
        #[serde(default)]
        entry: Vec<RunnableEntry>,
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "note: no whitelist at {} (treating as empty)",
                path.display()
            );
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.to_string()),
    };
    let whitelist_file: WhitelistFile = toml::from_str(&text).map_err(|error| error.to_string())?;
    Ok(whitelist_file.entry)
}

/// What one `ns-next run` or `load` of a whitelisted module produced.
struct RunOutcome {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// One way of running a row. Every row runs all four through the generated
/// wrapper application: `run` of its project file and `build` then `load` of
/// it, under each lowering -- the optimized one every program runs under, and
/// the reference lowering it is checked against (`docs/design/compiler.md`,
/// "The reference lowering"). No switch selects them, because a gate must not
/// be optional.
struct Lane {
    name: &'static str,
    lowering: &'static str,
    image: bool,
}

const LANES: [Lane; 4] = [
    Lane {
        name: "source",
        lowering: "optimized",
        image: false,
    },
    Lane {
        name: "image",
        lowering: "optimized",
        image: true,
    },
    Lane {
        name: "reference source",
        lowering: "reference",
        image: false,
    },
    Lane {
        name: "reference image",
        lowering: "reference",
        image: true,
    },
];

/// Runs `entry` every way `LANES` lists and checks each against the row's
/// recorded outcome: the exit status and stdout, and stderr -- empty for a
/// row expected to exit 0, and in every later lane equal to the first lane's.
/// The first lane that disagrees decides the row, and the message names it.
/// `executed` is set once the first lane's application has run at all, which
/// is what `ran` counts. One wrapper serves all four lanes.
fn check_row(
    executable: &Path,
    file: &Path,
    entry: &RunnableEntry,
    executed: &mut bool,
) -> Result<(), String> {
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "row".to_string());
    let scratch = ScratchDirectory::new(&stem)?;
    let project = write_wrapper(scratch.path(), file, &entry.send)?;
    let artifact = scratch.path().join("app.nsi");
    let mut first_stderr: Option<String> = None;
    for lane in &LANES {
        let outcome = if lane.image {
            run_one_binary(executable, &project, &artifact, lane.lowering)
        } else {
            run_one(executable, &project, lane.lowering)
        }
        .map_err(|error| format!("{}: {error}", lane.name))?;
        *executed = true;
        if outcome.status != Some(entry.expected_status) {
            return Err(format!(
                "{}: expected exit {}, got {} (stderr: {})",
                lane.name,
                entry.expected_status,
                match outcome.status {
                    Some(code) => code.to_string(),
                    None => "signal".to_string(),
                },
                outcome.stderr.trim_end()
            ));
        }
        if outcome.stdout != entry.expected_stdout {
            return Err(format!(
                "{}: expected stdout {:?}, got {:?}",
                lane.name, entry.expected_stdout, outcome.stdout
            ));
        }
        match &first_stderr {
            // A successful run writes nothing to stderr. Without this, a
            // warning every lane prints identically would pass, since later
            // lanes are compared only with the first.
            None if entry.expected_status == 0 && !outcome.stderr.is_empty() => {
                return Err(format!(
                    "{}: expected no stderr, got {:?}",
                    lane.name, outcome.stderr
                ));
            }
            None => first_stderr = Some(outcome.stderr),
            Some(first) if *first != outcome.stderr => {
                return Err(format!(
                    "{}: stderr differs from the {} run's: {:?}, against {:?}",
                    lane.name, LANES[0].name, outcome.stderr, first
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Writes the application that starts a corpus module and answers its project
/// file's path. The corpus modules are upstream's and declare no `main:args:`,
/// so a generated application instantiates the module the way its own primary
/// factory asks -- `new`, or `usingPlatform:` with the platform `main:args:`
/// receives -- and sends it the row's selector. `main:args:` answers what that
/// send answered and prints nothing itself, so a row's recorded stdout is the
/// module's alone.
///
/// Only a unary selector is sent: a keyword or binary one would need
/// arguments the wrapper would have to invent.
fn write_wrapper(scratch: &Path, module: &Path, send: &str) -> Result<PathBuf, String> {
    let is_unary = send
        .chars()
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && send
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_');
    if !is_unary {
        return Err(format!("run-corpus sends a unary selector, not `{send}`"));
    }
    let source = std::fs::read_to_string(module)
        .map_err(|error| format!("could not read {}: {error}", module.display()))?;
    let parsed =
        ns_syntax::parse(&source).map_err(|_| format!("{} does not parse", module.display()))?;
    let class_name = &parsed.root_class.name.name;
    let factory = parsed.root_class.factory.as_ref().map(selector_of);
    let instance = match factory.as_deref() {
        None | Some("new") => "Module new".to_string(),
        Some("usingPlatform:") => "Module usingPlatform: platform".to_string(),
        Some(other) => {
            return Err(format!(
                "the corpus module's primary factory `{other}` is neither `new` nor `usingPlatform:`"
            ));
        }
    };
    let wrapper = format!(
        "class RunCorpusEntry packageUsing: manifest = Object new (\n    \
         | private Module = manifest {class_name}. |\n) (\n    \
         public main: platform args: arguments = (\n        ({instance}) {send}\n    )\n) : ()\n"
    );
    std::fs::write(scratch.join("RunCorpusEntry.ns"), wrapper)
        .map_err(|error| format!("could not write the wrapper: {error}"))?;
    let absolute = std::path::absolute(module)
        .map_err(|error| format!("could not resolve {}: {error}", module.display()))?;
    let project = scratch.join(crate::project::PROJECT_FILE_NAME);
    let contents = serde_json::json!({
        "main": "RunCorpusEntry.ns",
        "modules": [absolute.to_string_lossy()],
    });
    std::fs::write(&project, contents.to_string())
        .map_err(|error| format!("could not write the project file: {error}"))?;
    Ok(project)
}

/// `stderr` without the line a child `ns-next run` or `load` begins it
/// with when the JIT is off (`entry::report_jit_unavailable`), which says
/// nothing of the module, whose recorded stderr it would never match:
/// printed once by this process instead, the first time a child prints it.
fn without_jit_warning(stderr: String) -> String {
    static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Some(rest) = stderr.strip_prefix(crate::entry::JIT_OFF_WARNING) else {
        return stderr;
    };
    let (reason, after) = rest.split_once('\n').unwrap_or((rest, ""));
    if !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("{}{}", crate::entry::JIT_OFF_WARNING, reason.trim_end());
    }
    after.to_string()
}

/// Runs the application through a *child* `ns-next run`.
///
/// A child process rather than an in-process call on purpose: the module's
/// stdout, which Newspeak code writes straight to the process's stdout
/// through `platform transcript`, is what gets compared, and running it in
/// this process would interleave it with the gate's own output.
fn run_one(executable: &Path, project: &Path, lowering: &str) -> Result<RunOutcome, String> {
    let out = crate::target_runner::target_command(executable)
        .arg("run")
        .arg(project)
        .arg("--lowering")
        .arg(lowering)
        .output()
        .map_err(|error| format!("could not run {}: {error}", executable.display()))?;
    Ok(RunOutcome {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: without_jit_warning(String::from_utf8_lossy(&out.stderr).into_owned()),
    })
}

/// A scratch directory for one row's binary lane, removed on drop so a
/// `run-corpus` run leaves the temporary directory clean on every exit path --
/// including the early `?`/`continue` returns scattered through the caller --
/// without every call site having to remember to clean up itself.
struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    /// `tag` is only for a human reading the temporary directory mid-run; the
    /// counter is what actually guarantees uniqueness across the rows this
    /// process runs, alongside the process id that separates concurrent
    /// `run-corpus` invocations.
    fn new(tag: &str) -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ns-next-run-corpus-{}-{sequence}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("scratch dir {}: {error}", directory.display()))?;
        Ok(Self(directory))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        // Best-effort: a leftover temporary directory is a nuisance, never a
        // correctness problem, so a removal failure (e.g. a virus scanner
        // still holding the file open) must not turn into a panic-during-
        // unwind or a spurious second failure report for the row.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the application the way a deployed one would: `build` it into the
/// scratch artifact, then `load` that, so the round trip through the artifact
/// is what is checked, not a second `run`.
///
/// The artifact lives in the row's scratch directory under the system
/// temporary directory, never under `corpus/`, so a `run-corpus` invocation
/// leaves the working tree exactly as it found it; `ScratchDirectory`'s `Drop`
/// removes it once the row is done whether it passed or failed.
fn run_one_binary(
    executable: &Path,
    project: &Path,
    artifact: &Path,
    lowering: &str,
) -> Result<RunOutcome, String> {
    let build = crate::target_runner::target_command(executable)
        .arg("build")
        .arg(project)
        .arg(artifact)
        .arg("--lowering")
        .arg(lowering)
        .output()
        .map_err(|error| format!("could not build {}: {error}", executable.display()))?;
    if !build.status.success() {
        return Err(format!(
            "build failed: {}",
            String::from_utf8_lossy(&build.stderr).trim_end()
        ));
    }

    let out = crate::target_runner::target_command(executable)
        .arg("load")
        .arg(artifact)
        .output()
        .map_err(|error| format!("could not load {}: {error}", executable.display()))?;
    Ok(RunOutcome {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: without_jit_warning(String::from_utf8_lossy(&out.stderr).into_owned()),
    })
}

/// `ns-next run-corpus`: the counterpart of `parse-corpus`, which requires
/// every module to parse; running one is not expected everywhere, so this
/// works off a whitelist -- a module
/// counts as running only once someone has recorded what running it
/// produces. Growth is therefore deliberate and a regression is loud.
///
/// Every entry runs the four ways `LANES` lists -- `run` of the generated
/// wrapper's project file and `build` then `load` of it, under each lowering --
/// and each is checked against the recorded exit status and stdout; stderr is
/// empty for a row expected to exit 0 and identical across lanes. Each row
/// still counts once in `ran`/`failed`: the first lane that fails decides the
/// row.
///
/// What that is worth, stated honestly: with `expected_stdout` empty on
/// every real row -- no corpus module that currently runs writes to the
/// transcript at all -- this is a tripwire that a whitelisted module still
/// instantiates and still answers one message without error, plus a check
/// that the same module still starts from a built image of the wrapper
/// application instead of from source. It is not yet a differential harness
/// over observable *output* from real modules; it becomes one on the day a row can record something
/// printed. Treat a green `run-corpus` as "nothing regressed", not as "the
/// corpus is covered".
pub(crate) fn execute(directory: &Path, whitelist: Option<&Path>) -> ExitCode {
    let whitelist_path = whitelist.map(PathBuf::from).unwrap_or_else(|| {
        directory
            .parent()
            .unwrap_or(directory)
            .join("runnable.toml")
    });
    let entries = match load_whitelist(&whitelist_path) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("error: whitelist {}: {error}", whitelist_path.display());
            return ExitCode::FAILURE;
        }
    };
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("error: cannot locate this executable to re-run modules with: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut present: Vec<PathBuf> = Vec::new();
    collect_ns_files(directory, &mut present);
    let listed: std::collections::HashSet<&str> =
        entries.iter().map(|entry| entry.file.as_str()).collect();
    let skipped = present
        .iter()
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| listed.contains(name.to_string_lossy().as_ref()))
        })
        .count();

    // `ran` counts rows whose module actually executed, so a row naming a
    // deleted file, or one whose child could not be spawned, reports as a
    // failure without inflating the number a reader uses to judge coverage.
    let (mut ran, mut failed) = (0, 0);
    for entry in &entries {
        let file = directory.join(&entry.file);
        if !file.is_file() {
            failed += 1;
            eprintln!(
                "FAIL {}: no such file under {}",
                entry.file,
                directory.display()
            );
            continue;
        }
        let mut executed = false;
        let result = check_row(&executable, &file, entry, &mut executed);
        if executed {
            ran += 1;
        }
        if let Err(error) = result {
            failed += 1;
            eprintln!("FAIL {}: {error}", entry.file);
        }
    }
    println!("ran {ran}, failed {failed}, skipped {skipped}");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
