//! What keeps the MCP server and its programs from locking the build
//! (`docs/design/debugger.md`, "The MCP server", "Starting it"): on
//! Windows a running executable can be neither replaced nor removed, and
//! neither the server nor a session may keep `cargo build` from writing
//! `target`. The server stays the process its client started -- a client
//! watches that process, and Node's `child_process`, on which the MCP
//! SDKs' stdio transports are built, closes the server's input when it
//! ends -- so on Windows it renames its own executable aside, which
//! Windows permits for a running image within its volume, and puts a
//! copy back at the original path, which a build then overwrites; every
//! program a session starts on Windows runs a copy in the user's own local
//! data directory, in a directory named by a hash of the binary's bytes,
//! the copy checked against the binary's bytes before it runs; elsewhere
//! the binary itself. And the shipped sources of the binary a session
//! runs, written once per fingerprint in the user's own directory, which
//! a stop names its shipped frames' files in. A server that starts
//! removes what lay there untouched for a day, but its own binary's
//! sources. Nothing is kept in the
//! shared temporary directory, where another user could put a file of
//! their own first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::target_runner;

/// The directory in the user's own local data directory that holds what
/// the server keeps (`own_directory`).
const OWN_DIRECTORY: &str = "ns-next";

/// The directory below `OWN_DIRECTORY` that holds the copies of
/// executables, one directory per hash.
const COPIES_DIRECTORY: &str = "mcp-copies";

/// The directory below `OWN_DIRECTORY` that holds the shipped sources, one
/// directory per fingerprint.
const CORE_LIBRARY_DIRECTORY: &str = "core-library";

/// The suffix of an executable a server renamed aside: `<file
/// name>.<process id>.running`, beside the original.
const RUNNING_SUFFIX: &str = ".running";

/// How long a directory of copies, or of shipped sources, stays untouched
/// before a server that starts removes it: each build is a copy of its
/// own, each change of the core library a directory of sources, and a
/// day is longer than any session. Copies are made only on Windows, where
/// one a process still runs cannot be removed; no process runs from a
/// directory of sources, and a session that reuses one touches it.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);

/// The file of a directory of shipped sources that names their
/// fingerprint, which `shipped-sources` writes.
const FINGERPRINT_FILE: &str = "fingerprint";

/// Frees the executable this server was started from, and answers its
/// path: the build a `debug_start` runs by default. On Windows the file is
/// renamed aside and a copy of it put back at its path, so that a build
/// replaces a file no process runs, after the files earlier servers
/// renamed aside that no process runs any more are removed; elsewhere a
/// running file can be replaced as it is. What fails is said on the
/// standard error, and the server serves all the same.
pub(crate) fn free_the_executable() -> PathBuf {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("ns-next mcp: the executable's path is unknown: {error}");
            return PathBuf::from("ns-next");
        }
    };
    remove_stale_copies();
    remove_stale_core_libraries();
    if cfg!(windows) {
        remove_unused_renamed(&executable);
        if let Err(error) = rename_aside(&executable) {
            eprintln!(
                "ns-next mcp: {} stays in use: {error}",
                executable.display()
            );
        }
    }
    executable
}

/// Renames `executable` to `<file name>.<process id>.running` beside it
/// and copies it back to its path.
fn rename_aside(executable: &Path) -> Result<(), String> {
    let Some(name) = executable.file_name() else {
        return Err("it names no file".to_string());
    };
    let mut aside_name = name.to_os_string();
    aside_name.push(format!(".{}{RUNNING_SUFFIX}", std::process::id()));
    let aside = executable.with_file_name(aside_name);
    std::fs::rename(executable, &aside)
        .map_err(|error| format!("renaming it to {}: {error}", aside.display()))?;
    // Written under a name of this process's and renamed into place, so
    // that a process started from the path meanwhile never meets a half
    // written file; another server started from the same path may have
    // put its copy back first, which is the same build.
    let mut partial_name = name.to_os_string();
    partial_name.push(format!(".{}.partial", std::process::id()));
    let partial = executable.with_file_name(partial_name);
    let restored =
        std::fs::copy(&aside, &partial).and_then(|_| std::fs::rename(&partial, executable));
    if let Err(error) = restored {
        let _ = std::fs::remove_file(&partial);
        if !executable.is_file() {
            // The path never stays empty: the running file goes back,
            // and the build stays locked out as it was before.
            let _ = std::fs::rename(&aside, executable);
            return Err(format!("copying it back: {error}"));
        }
    }
    Ok(())
}

/// Removes the files beside `executable` that a server renamed aside:
/// the removal of one a process still runs fails, which leaves it.
fn remove_unused_renamed(executable: &Path) {
    let (Some(directory), Some(name)) = (executable.parent(), executable.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        let renamed = file_name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(RUNNING_SUFFIX))
            .is_some_and(|process| {
                !process.is_empty() && process.bytes().all(|byte| byte.is_ascii_digit())
            });
        if renamed {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The directory the server keeps its files in, the user's own: on
/// Windows `ns-next` in the local application data directory
/// (`%LOCALAPPDATA%`), elsewhere in the user's cache directory
/// (`$XDG_CACHE_HOME`, or `~/.cache`). Never the shared temporary
/// directory, where another user could put a file of their own first --
/// a copy the server would run, or sources it would show.
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

/// What a session runs for `executable`: on Windows, where a running file
/// can be neither replaced nor removed, its copy in the directory its
/// bytes' hash names, made when it is not there yet and written again when
/// the file there is not the executable's bytes; elsewhere, where a build
/// replaces a running file as it is, the executable itself. The copy is
/// written under a name of this process's and renamed into place, so that
/// no process ever runs a half written copy, and two servers that copy at
/// once both find a whole one.
pub(crate) fn copy_of(executable: &Path) -> Result<PathBuf, String> {
    if !cfg!(windows) {
        return Ok(executable.to_path_buf());
    }
    let hash = hash_of_file(executable)
        .map_err(|error| format!("reading {}: {error}", executable.display()))?;
    let Some(name) = executable.file_name() else {
        return Err(format!("{} names no file", executable.display()));
    };
    let directory = own_directory()?
        .join(COPIES_DIRECTORY)
        .join(format!("{hash:016x}"));
    let copy = directory.join(name);
    if same_bytes(executable, &copy) {
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
        if !same_bytes(executable, &copy) {
            return Err(format!("writing {}: {error}", copy.display()));
        }
    }
    Ok(copy)
}

/// Whether the file `copy` holds exactly the bytes of `executable`: the
/// hash that names the copy's directory says nothing of what the file in
/// it holds, which a crash or anyone with access may have left otherwise.
fn same_bytes(executable: &Path, copy: &Path) -> bool {
    use std::io::Read;
    let open = |path: &Path| {
        std::fs::File::open(path).map(|file| std::io::BufReader::with_capacity(1 << 20, file))
    };
    let (Ok(mut original), Ok(mut copied)) = (open(executable), open(copy)) else {
        return false;
    };
    let mut original_chunk = vec![0; 1 << 16];
    let mut copied_chunk = vec![0; 1 << 16];
    loop {
        let Ok(read) = original.read(&mut original_chunk) else {
            return false;
        };
        if read == 0 {
            // The copy must end where the executable does.
            return copied
                .read(&mut copied_chunk[..1])
                .is_ok_and(|more| more == 0);
        }
        if copied.read_exact(&mut copied_chunk[..read]).is_err()
            || original_chunk[..read] != copied_chunk[..read]
        {
            return false;
        }
    }
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

/// Removes the directories of copies untouched for `STALE`; what cannot be
/// removed is left.
fn remove_stale_copies() {
    let Ok(own) = own_directory() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(own.join(COPIES_DIRECTORY)) else {
        return;
    };
    for entry in entries.flatten() {
        if older_than_stale(&entry.path()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Removes the directories of shipped sources untouched for `STALE` but
/// the running binary's, which the sessions of this server most likely
/// show: a stale one is never read again but by a session of another
/// binary, which writes it anew. A directory's last touch is its
/// fingerprint file's, which `core_library` touches at every reuse, or,
/// without one -- a partial write a crash left -- the directory's own.
/// What cannot be removed is left.
fn remove_stale_core_libraries() {
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
