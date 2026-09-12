//! `ns-next run-corpus`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::source::collect_ns_files;

/// One row of `corpus/runnable.toml`: a corpus module that is expected to
/// run, with everything needed to check that it still does.
///
/// `send` is per entry rather than hardcoded to `main:args:` because the
/// corpus modules that run today have no `main:args:` -- they are library
/// modules whose reachable entry point is a public method or a public
/// nested class's implicit accessor. A gate that assumed one selector
/// could not list any of them.
#[derive(serde::Deserialize)]
struct RunnableEntry {
    file: String,
    send: String,
    expected_status: i32,
    expected_stdout: String,
    /// Why this module is on the list. Required, like the blacklist's own
    /// `reason`: the list is meant to grow one deliberate row at a time.
    #[allow(dead_code)]
    reason: String,
}

fn load_whitelist(path: &Path) -> Result<Vec<RunnableEntry>, String> {
    #[derive(serde::Deserialize)]
    struct Wl {
        #[serde(default)]
        entry: Vec<RunnableEntry>,
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "note: no whitelist at {} (treating as empty)",
                path.display()
            );
            return Ok(Vec::new());
        }
        Err(e) => return Err(e.to_string()),
    };
    let wl: Wl = toml::from_str(&text).map_err(|e| e.to_string())?;
    Ok(wl.entry)
}

/// What one `ns-next run` or `load` of a whitelisted module produced.
struct RunOutcome {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// One way of running a row. Every row runs all four: from source and as a
/// built binary, under each lowering -- the optimized one every program runs
/// under, and the reference lowering it is checked against
/// (`docs/design/compiler.md`, "The reference lowering"). No switch selects
/// them, because a gate must not be optional.
struct Lane {
    name: &'static str,
    lowering: &'static str,
    binary: bool,
}

const LANES: [Lane; 4] = [
    Lane {
        name: "source",
        lowering: "optimized",
        binary: false,
    },
    Lane {
        name: "binary",
        lowering: "optimized",
        binary: true,
    },
    Lane {
        name: "reference source",
        lowering: "reference",
        binary: false,
    },
    Lane {
        name: "reference binary",
        lowering: "reference",
        binary: true,
    },
];

/// Runs `entry` every way `LANES` lists and checks each against the row's
/// recorded outcome: the exit status and stdout, and stderr -- empty for a
/// row expected to exit 0, and in every later lane equal to the first lane's.
/// The first lane that disagrees decides the row, and the message names it.
/// `executed` is set once the first lane's module has run at all, which is
/// what `ran` counts.
fn check_row(
    exe: &Path,
    file: &Path,
    entry: &RunnableEntry,
    executed: &mut bool,
) -> Result<(), String> {
    let mut first_stderr: Option<String> = None;
    for lane in &LANES {
        let outcome = if lane.binary {
            run_one_binary(exe, file, &entry.send, lane.lowering)
        } else {
            run_one(exe, file, &entry.send, lane.lowering)
        }
        .map_err(|e| format!("{}: {e}", lane.name))?;
        *executed = true;
        if outcome.status != Some(entry.expected_status) {
            return Err(format!(
                "{}: expected exit {}, got {} (stderr: {})",
                lane.name,
                entry.expected_status,
                match outcome.status {
                    Some(c) => c.to_string(),
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

/// Runs one module through a *child* `ns-next run`.
///
/// A child process rather than an in-process call on purpose: the module's
/// stdout, which Newspeak code writes straight to the process's stdout
/// through `platform transcript`, is what gets compared, and running it in
/// this process would interleave it with the gate's own output.
fn run_one(exe: &Path, file: &Path, send: &str, lowering: &str) -> Result<RunOutcome, String> {
    let out = std::process::Command::new(exe)
        .arg("run")
        .arg(file)
        .arg("--send")
        .arg(send)
        .arg("--lowering")
        .arg(lowering)
        .output()
        .map_err(|e| format!("could not run {}: {e}", exe.display()))?;
    Ok(RunOutcome {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// A scratch directory for one row's binary lane, removed on drop so a
/// `run-corpus` run leaves the temp directory clean on every exit path --
/// including the early `?`/`continue` returns scattered through the caller --
/// without every call site having to remember to clean up itself.
struct ScratchDir(PathBuf);

impl ScratchDir {
    /// `tag` is only for a human reading the temp directory mid-run; the
    /// counter is what actually guarantees uniqueness across the rows this
    /// process runs, alongside the process id that separates concurrent
    /// `run-corpus` invocations.
    fn new(tag: &str) -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ns-next-run-corpus-{}-{n}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("scratch dir {}: {e}", dir.display()))?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best-effort: a leftover temp directory is a nuisance, never a
        // correctness problem, so a removal failure (e.g. a virus scanner
        // still holding the file open) must not turn into a panic-during-
        // unwind or a spurious second failure report for the row.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs one module the way a deployed binary would: `build` it with
/// `--entry <send>` into a scratch file, then `load` it with no `--send` so
/// the artifact's own recorded entry is what starts it. That is deliberate,
/// not an oversight -- supplying `--send` again here would exercise the same
/// override path `build_load.rs` already covers, not the round trip this
/// gate exists to check: that the entry survives being written into the
/// artifact and read back out of it.
///
/// The scratch file lives under the system temp directory, never under
/// `corpus/`, so a `run-corpus` invocation leaves the working tree exactly
/// as it found it; `ScratchDir`'s `Drop` removes it once this row is done
/// whether the row passed or failed.
fn run_one_binary(
    exe: &Path,
    file: &Path,
    send: &str,
    lowering: &str,
) -> Result<RunOutcome, String> {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "row".to_string());
    let scratch = ScratchDir::new(&stem)?;
    let artifact = scratch.path().join("app.nsb");

    let build = std::process::Command::new(exe)
        .arg("build")
        .arg(file)
        .arg(&artifact)
        .arg("--entry")
        .arg(send)
        .arg("--lowering")
        .arg(lowering)
        .output()
        .map_err(|e| format!("could not build {}: {e}", exe.display()))?;
    if !build.status.success() {
        return Err(format!(
            "build failed: {}",
            String::from_utf8_lossy(&build.stderr).trim_end()
        ));
    }

    let out = std::process::Command::new(exe)
        .arg("load")
        .arg(&artifact)
        .output()
        .map_err(|e| format!("could not load {}: {e}", exe.display()))?;
    Ok(RunOutcome {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// `ns-next run-corpus`: the mirror image of `parse-corpus`. `parse-corpus`
/// works off a blacklist because parsing the corpus is expected to succeed
/// everywhere; running it is not, so this works off a whitelist -- a module
/// counts as running only once someone has recorded what running it
/// produces. Growth is therefore deliberate and a regression is loud.
///
/// Every entry runs the four ways `LANES` lists -- from source and as a built
/// binary, under each lowering -- and each is checked against the recorded
/// exit status and stdout; stderr is empty for a row expected to exit 0 and
/// identical across lanes. A row's `send` doubles as the build's `--entry`,
/// which is why `runnable.toml` needs no field of its own for the binary
/// lanes. Each row still counts once in `ran`/`failed`: the first lane that
/// fails decides the row.
///
/// What that is worth, stated honestly: with `expected_stdout` empty on
/// every real row -- no corpus module that currently runs writes to the
/// transcript at all -- this is a tripwire that a whitelisted module still
/// instantiates and still answers one message without error, plus a check
/// that the same module still starts from a built binary instead of from
/// source. It is not yet a differential harness over observable *output*
/// from real modules; it becomes one on the day a row can record something
/// printed. Treat a green `run-corpus` as "nothing regressed", not as "the
/// corpus is covered".
pub(crate) fn execute(dir: &Path, whitelist: Option<&Path>) -> ExitCode {
    let wl_path = whitelist
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.parent().unwrap_or(dir).join("runnable.toml"));
    let entries = match load_whitelist(&wl_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: whitelist {}: {e}", wl_path.display());
            return ExitCode::FAILURE;
        }
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot locate this executable to re-run modules with: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut present: Vec<PathBuf> = Vec::new();
    collect_ns_files(dir, &mut present);
    let listed: std::collections::HashSet<&str> = entries.iter().map(|e| e.file.as_str()).collect();
    let skipped = present
        .iter()
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| listed.contains(n.to_string_lossy().as_ref()))
        })
        .count();

    // `ran` counts rows whose module actually executed, so a row naming a
    // deleted file, or one whose child could not be spawned, reports as a
    // failure without inflating the number a reader uses to judge coverage.
    let (mut ran, mut failed) = (0, 0);
    for entry in &entries {
        let file = dir.join(&entry.file);
        if !file.is_file() {
            failed += 1;
            eprintln!("FAIL {}: no such file under {}", entry.file, dir.display());
            continue;
        }
        let mut executed = false;
        let result = check_row(&exe, &file, entry, &mut executed);
        if executed {
            ran += 1;
        }
        if let Err(e) = result {
            failed += 1;
            eprintln!("FAIL {}: {e}", entry.file);
        }
    }
    println!("ran {ran}, failed {failed}, skipped {skipped}");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
