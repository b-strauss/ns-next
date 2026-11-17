//! `cargo xtask mcp`: starts the repository's MCP server, `ns-next mcp`
//! (`docs/design/debugger.md`, "The MCP server", "Starting it"), from a
//! copy of the last debug build, so that neither the server nor the
//! programs it debugs keep `cargo build` from replacing
//! `target/debug/ns-next` -- on Windows a running executable can be
//! neither replaced nor removed. The binary knows nothing of the build;
//! the repository's own concern is here. The task builds nothing: a
//! build it started would write to the standard output the client reads
//! and keep the client waiting, so the developer builds, and the next
//! connection runs what was built.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

/// The directory below the target directory that holds the copies.
const COPIES_DIRECTORY: &str = "mcp";

/// The name of the executable, the build's and its copy's.
fn executable_name() -> String {
    format!("ns-next{}", std::env::consts::EXE_SUFFIX)
}

/// How long a partial copy of another process's stays before a task
/// removes it: far longer than a copy takes, so that no task removes one
/// another task is still writing.
const PARTIAL_STALE: Duration = Duration::from_secs(60);

/// The suffix of a copy being written, before it is renamed into place.
const PARTIAL_SUFFIX: &str = ".partial";

/// Copies the debug build into the copies' directory and runs the copy as
/// `ns-next mcp`, its standard streams this process's own, so that the
/// client speaks to the server directly; answers its exit status. What
/// fails before the server runs is said on the standard error, which the
/// client shows, never on the standard output, which the protocol owns.
pub(crate) fn serve(root: &Path) -> ExitCode {
    let target = target_directory(root);
    let build = target.join("debug").join(executable_name());
    if !build.is_file() {
        eprintln!(
            "cargo xtask mcp: there is no build of ns-next at {}; build it with \
             `cargo build -p ns-cli` and connect again",
            build.display()
        );
        return ExitCode::FAILURE;
    }
    let copy = match place_copy(&build, &target.join(COPIES_DIRECTORY), std::process::id()) {
        Ok(copy) => copy,
        Err(failure) => {
            eprintln!("cargo xtask mcp: {failure}");
            return ExitCode::FAILURE;
        }
    };
    let status = Command::new(&copy)
        .arg("mcp")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    match status {
        Ok(status) => status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        Err(error) => {
            eprintln!("cargo xtask mcp: starting {}: {error}", copy.display());
            ExitCode::FAILURE
        }
    }
}

/// The target directory cargo builds into: `CARGO_TARGET_DIR` when it is
/// set, a relative one taken from the current directory as cargo takes
/// it, and `target` in the repository's root otherwise.
fn target_directory(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || root.join("target"),
        |directory| std::path::absolute(&directory).unwrap_or_else(|_| PathBuf::from(directory)),
    )
}

/// Puts a copy of `build` into `directory` and answers its path: first
/// the copies no process runs any more are removed -- on Windows the
/// removal of one a process runs fails, which leaves it -- then the copy
/// is written under a name of `process`'s and renamed to the
/// executable's name, or, when an earlier server still runs a file of
/// that name, which Windows does not let a rename replace, to a name of
/// `process`'s own. No process ever runs a half written copy.
fn place_copy(build: &Path, directory: &Path, process: u32) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("creating {}: {error}", directory.display()))?;
    remove_unused_copies(directory);
    let name = executable_name();
    let partial = directory.join(format!("{name}.{process}{PARTIAL_SUFFIX}"));
    std::fs::copy(build, &partial).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        format!(
            "copying {} to {}: {error}",
            build.display(),
            partial.display()
        )
    })?;
    let copy = directory.join(&name);
    if std::fs::rename(&partial, &copy).is_ok() {
        return Ok(copy);
    }
    let own = directory.join(format!("ns-next-{process}{}", std::env::consts::EXE_SUFFIX));
    std::fs::rename(&partial, &own).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        format!("renaming the copy to {}: {error}", own.display())
    })?;
    Ok(own)
}

/// Removes every file in `directory` but a partial copy another task may
/// still be writing; a removal that fails -- on Windows, of a copy a
/// process runs -- leaves the file.
fn remove_unused_copies(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let partial = entry
            .file_name()
            .to_string_lossy()
            .ends_with(PARTIAL_SUFFIX);
        if partial && !older_than(&path, PARTIAL_STALE) {
            continue;
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// Whether `path` was last modified more than `age` ago; `false` when its
/// age is unknown.
fn older_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory of the test's own below the system's temporary
    /// directory, removed when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path = std::env::temp_dir()
                .join(format!("ns-next-xtask-mcp-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create the scratch directory");
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names_in(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .expect("read the directory")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    /// The copy holds the build's bytes under the executable's name, and
    /// what earlier tasks left that no process holds is removed.
    #[test]
    fn the_copy_replaces_what_no_process_holds() {
        let scratch = Scratch::new("replaces");
        let build = scratch.0.join("build");
        std::fs::write(&build, "the new build").expect("write the build");
        let directory = scratch.0.join("mcp");
        std::fs::create_dir_all(&directory).expect("create the copies' directory");
        std::fs::write(directory.join(executable_name()), "an old build")
            .expect("write an old copy");
        std::fs::write(directory.join("ns-next-7"), "an older server's").expect("write a leftover");
        let copy = place_copy(&build, &directory, 42).expect("a copy");
        assert_eq!(copy, directory.join(executable_name()));
        assert_eq!(
            std::fs::read_to_string(&copy).expect("read the copy"),
            "the new build"
        );
        assert_eq!(names_in(&directory), [executable_name()]);
    }

    /// A copy an earlier server still runs stays, and the new one takes a
    /// name of its process's own. Windows refuses to replace or remove a
    /// running image as it refuses a file opened without sharing its
    /// deletion, which the test holds the copy as; elsewhere a running
    /// file is replaced like any other.
    #[test]
    fn a_copy_in_use_is_left_and_the_new_one_named_by_its_process() {
        let scratch = Scratch::new("in-use");
        let build = scratch.0.join("build");
        std::fs::write(&build, "the new build").expect("write the build");
        let directory = scratch.0.join("mcp");
        std::fs::create_dir_all(&directory).expect("create the copies' directory");
        let held = directory.join(executable_name());
        std::fs::write(&held, "a running build").expect("write the running copy");
        let holder = hold(&held);
        let copy = place_copy(&build, &directory, 42).expect("a copy");
        assert_eq!(
            std::fs::read_to_string(&copy).expect("read the copy"),
            "the new build"
        );
        if cfg!(windows) {
            let own = format!("ns-next-42{}", std::env::consts::EXE_SUFFIX);
            assert_eq!(copy, directory.join(&own));
            assert_eq!(
                std::fs::read_to_string(&held).expect("read the running copy"),
                "a running build"
            );
            drop(holder);
            // The next task removes it, now that nothing holds it.
            let next = place_copy(&build, &directory, 43).expect("a copy");
            assert_eq!(next, directory.join(executable_name()));
            assert_eq!(names_in(&directory), [executable_name()]);
        } else {
            assert_eq!(copy, held);
            drop(holder);
        }
    }

    /// Opens `path` as a running image holds it: on Windows sharing only
    /// its reading, so that neither a removal nor a rename over it succeeds.
    fn hold(path: &Path) -> std::fs::File {
        let mut options = std::fs::File::options();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            /// `FILE_SHARE_READ`: others may read the file, but neither
            /// write nor delete it.
            const SHARE_READ: u32 = 1;
            options.share_mode(SHARE_READ);
        }
        options.open(path).expect("hold the copy")
    }

    /// A partial copy another task may still be writing stays.
    #[test]
    fn a_fresh_partial_copy_of_another_task_s_stays() {
        let scratch = Scratch::new("partial");
        let build = scratch.0.join("build");
        std::fs::write(&build, "the new build").expect("write the build");
        let directory = scratch.0.join("mcp");
        std::fs::create_dir_all(&directory).expect("create the copies' directory");
        let partial = format!("{}.7{PARTIAL_SUFFIX}", executable_name());
        std::fs::write(directory.join(&partial), "half").expect("write a partial copy");
        place_copy(&build, &directory, 42).expect("a copy");
        let mut expected = vec![executable_name(), partial];
        expected.sort();
        assert_eq!(names_in(&directory), expected);
    }
}
