//! The shipped sources of the binary a session runs, written once per
//! fingerprint in the user's own directory, which a stop names its
//! shipped frames' files in (`docs/design/debugger.md`, "The MCP
//! server"), so that a frame in `Kernel.ns` names a file whose text is
//! the running binary's. A server that starts removes the directories
//! that lay there untouched for a day, but its own binary's. Nothing is
//! kept in the shared temporary directory, where another user could put
//! a file of their own first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::target_runner;

/// The directory in the user's own local data directory that holds what
/// the server keeps (`own_directory`).
const OWN_DIRECTORY: &str = "ns-next";

/// The directory below `OWN_DIRECTORY` that holds the shipped sources, one
/// directory per fingerprint.
const CORE_LIBRARY_DIRECTORY: &str = "core-library";

/// How long a directory of shipped sources stays untouched before a
/// server that starts removes it: each change of the core library is a
/// directory of its own, and a day is longer than any session. No process
/// runs from a directory of sources, and a session that reuses one
/// touches it.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);

/// The file of a directory of shipped sources that names their
/// fingerprint, which `shipped-sources` writes.
const FINGERPRINT_FILE: &str = "fingerprint";

/// The directory the server keeps its files in, the user's own: on
/// Windows `ns-next` in the local application data directory
/// (`%LOCALAPPDATA%`), elsewhere in the user's cache directory
/// (`$XDG_CACHE_HOME`, or `~/.cache`). Never the shared temporary
/// directory, where another user could put a file of their own first --
/// sources the server would show.
fn own_directory() -> Result<PathBuf, String> {
    let absolute = |variable: &str| {
        std::env::var_os(variable)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let base = if cfg!(windows) {
        absolute("LOCALAPPDATA").ok_or("LOCALAPPDATA names no directory")?
    } else {
        absolute("XDG_CACHE_HOME")
            .or_else(|| absolute("HOME").map(|home| home.join(".cache")))
            .ok_or("neither XDG_CACHE_HOME nor HOME names a directory")?
    };
    Ok(base.join(OWN_DIRECTORY))
}

/// Removes the directories of shipped sources untouched for `STALE` but
/// the running binary's, which the sessions of this server most likely
/// show: a stale one is never read again but by a session of another
/// binary, which writes it anew. A directory's last touch is its
/// fingerprint file's, which `core_library` touches at every reuse, or,
/// without one -- a partial write a crash left -- the directory's own.
/// What cannot be removed is left.
pub(crate) fn remove_stale_core_libraries() {
    let Ok(own) = own_directory() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(own.join(CORE_LIBRARY_DIRECTORY)) else {
        return;
    };
    let running = ns_runtime::shipped_sources_fingerprint();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy() == running {
            continue;
        }
        let fingerprint_file = path.join(FINGERPRINT_FILE);
        let touched = if fingerprint_file.is_file() {
            fingerprint_file
        } else {
            path.clone()
        };
        if older_than_stale(&touched) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Whether `path` was last modified more than `STALE` ago; `false` when its
/// age is unknown.
fn older_than_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > STALE)
}

/// The shipped sources of the binary `runtime` and their fingerprint: the
/// directory in the user's own directory (`own_directory`) its fingerprint
/// names, written by `runtime shipped-sources` when it is not there yet,
/// under a name of this process's and renamed into place, and touched when
/// it is, so that no server removes it as stale
/// (`remove_stale_core_libraries`).
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
    let root = own_directory()?.join(CORE_LIBRARY_DIRECTORY);
    let directory = root.join(&fingerprint);
    if written_with(&directory, &fingerprint) {
        let _ = std::fs::File::options()
            .write(true)
            .open(directory.join(FINGERPRINT_FILE))
            .and_then(|file| file.set_modified(std::time::SystemTime::now()));
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
    std::fs::read_to_string(directory.join(FINGERPRINT_FILE))
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
