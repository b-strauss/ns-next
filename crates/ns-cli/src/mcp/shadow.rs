//! The executable copies the MCP server runs from (`docs/design/debugger.md`,
//! "The MCP server", "Starting it"): the server and every program it
//! starts run a copy of their binary, below the temporary directory in a
//! directory named by a hash of the binary's bytes, never the binary the
//! build writes -- on Windows a running executable can be neither replaced
//! nor removed, and neither the server nor a session may keep `cargo build`
//! from writing `target`. And the shipped sources of the binary a session
//! runs, written once per fingerprint, which a stop names its shipped
//! frames' files in.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use crate::target_runner;

/// The variable through which a server started from its copy learns the
/// path it was started from: the build a `debug_start` runs by default.
/// Set, it also says the process already runs from its copy.
pub(crate) const EXECUTABLE_VARIABLE: &str = "NS_NEXT_MCP_EXECUTABLE";

/// The directory below the temporary directory that holds the copies of
/// executables, one directory per hash.
const COPIES_DIRECTORY: &str = "ns-next-mcp";

/// The directory below the temporary directory that holds the shipped
/// sources, one directory per fingerprint.
const CORE_LIBRARY_DIRECTORY: &str = "ns-next-core";

/// How long a directory of copies stays untouched before a server that
/// starts removes it: each build is a copy of its own, and a day is
/// longer than any session. One still in use cannot be removed on
/// Windows, and on other systems its running processes keep their file.
const STALE_COPIES: Duration = Duration::from_secs(24 * 60 * 60);

/// Starts the server again from a copy of its executable, unless it runs
/// from one already: answers the exit code of this process when the copy
/// took over, or `None` when this process is to serve -- the copy itself,
/// or the original when no copy could be made, which is said on the
/// standard error.
pub(crate) fn relaunch_from_copy() -> Option<ExitCode> {
    if std::env::var_os(EXECUTABLE_VARIABLE).is_some() {
        return None;
    }
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("ns-next mcp: serving from the build, its path unknown: {error}");
            return None;
        }
    };
    remove_stale_copies();
    let copy = match copy_of(&executable) {
        Ok(copy) => copy,
        Err(error) => {
            eprintln!("ns-next mcp: serving from the build, not a copy: {error}");
            return None;
        }
    };
    let mut command = target_runner::target_command(&copy);
    command.arg("mcp").env(EXECUTABLE_VARIABLE, &executable);
    hand_over(command)
}

/// Replaces this process by `command`, which serves on the same standard
/// streams.
#[cfg(unix)]
fn hand_over(mut command: Command) -> Option<ExitCode> {
    use std::os::unix::process::CommandExt;
    let error = command.exec();
    eprintln!("ns-next mcp: serving from the build, the copy did not start: {error}");
    None
}

/// Starts `command` on the same standard streams, which it inherits, and
/// lets this process end at once: Windows has no `exec`, and the original
/// file is free only once no process runs it. The client's pipes stay
/// open as long as the copy holds them.
#[cfg(not(unix))]
fn hand_over(mut command: Command) -> Option<ExitCode> {
    match command.spawn() {
        Ok(_) => Some(ExitCode::SUCCESS),
        Err(error) => {
            eprintln!("ns-next mcp: serving from the build, the copy did not start: {error}");
            None
        }
    }
}

/// The copy of `executable` in the directory its bytes' hash names, made
/// when it is not there yet. The copy is written under a name of this
/// process's and renamed into place, so that no process ever runs a half
/// written copy, and two servers that copy at once both find a whole one.
pub(crate) fn copy_of(executable: &Path) -> Result<PathBuf, String> {
    let hash = hash_of_file(executable)
        .map_err(|error| format!("reading {}: {error}", executable.display()))?;
    let Some(name) = executable.file_name() else {
        return Err(format!("{} names no file", executable.display()));
    };
    let directory = std::env::temp_dir()
        .join(COPIES_DIRECTORY)
        .join(format!("{hash:016x}"));
    let copy = directory.join(name);
    if copy.is_file() {
        return Ok(copy);
    }
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("creating {}: {error}", directory.display()))?;
    let mut partial_name = name.to_os_string();
    partial_name.push(format!(".{}.partial", std::process::id()));
    let partial = directory.join(partial_name);
    std::fs::copy(executable, &partial).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        format!("copying {}: {error}", executable.display())
    })?;
    if let Err(error) = std::fs::rename(&partial, &copy) {
        let _ = std::fs::remove_file(&partial);
        // Another server put the same bytes there first.
        if !copy.is_file() {
            return Err(format!("writing {}: {error}", copy.display()));
        }
    }
    Ok(copy)
}

/// FNV-1a over the file's bytes, the hash the shipped sources' fingerprint
/// takes (`ns_runtime::shipped_sources_fingerprint`): a name for the bytes,
/// not a guard against a forger, who could replace the copy as easily.
fn hash_of_file(path: &Path) -> std::io::Result<u64> {
    use std::io::Read;
    let mut file = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    let mut chunk = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(state);
        }
        for byte in &chunk[..read] {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// Removes the directories of copies untouched for `STALE_COPIES`; what
/// cannot be removed is left.
fn remove_stale_copies() {
    let root = std::env::temp_dir().join(COPIES_DIRECTORY);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_COPIES);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The shipped sources of the binary `runtime` and their fingerprint: the
/// directory below the temporary directory its fingerprint names, written
/// by `runtime shipped-sources` when it is not there yet, under a name of
/// this process's and renamed into place.
pub(crate) fn core_library(runtime: &Path) -> Result<(PathBuf, String), String> {
    let fingerprint = run(runtime, &["shipped-sources", "--fingerprint"])?;
    let fingerprint = fingerprint.trim().to_string();
    // It names a directory: anything but a hex number is refused.
    if fingerprint.is_empty() || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "{} answered no fingerprint of its shipped sources: {fingerprint:?}",
            runtime.display()
        ));
    }
    let root = std::env::temp_dir().join(CORE_LIBRARY_DIRECTORY);
    let directory = root.join(&fingerprint);
    if written_with(&directory, &fingerprint) {
        return Ok((directory, fingerprint));
    }
    let partial = root.join(format!("{fingerprint}.{}.partial", std::process::id()));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("creating {}: {error}", root.display()))?;
    let partial_text = partial.to_string_lossy().to_string();
    run(runtime, &["shipped-sources", &partial_text])?;
    if std::fs::rename(&partial, &directory).is_err() {
        let _ = std::fs::remove_dir_all(&partial);
        // Another server wrote the same sources there first.
        if !written_with(&directory, &fingerprint) {
            return Err(format!(
                "writing the shipped sources to {}",
                directory.display()
            ));
        }
    }
    Ok((directory, fingerprint))
}

/// Whether `directory` holds shipped sources of `fingerprint`.
fn written_with(directory: &Path, fingerprint: &str) -> bool {
    std::fs::read_to_string(directory.join("fingerprint"))
        .is_ok_and(|written| written.trim() == fingerprint)
}

/// What `runtime` with `arguments` writes to its standard output, or why
/// it failed.
fn run(runtime: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = target_runner::target_command(runtime)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("starting {}: {error}", runtime.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} {} failed ({}): {}",
            runtime.display(),
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
