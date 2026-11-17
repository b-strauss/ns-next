//! `cargo xtask mcp`: puts the copy of the debug build that `.mcp.json`
//! starts the repository's MCP server from, `target/mcp/ns-next`
//! (`docs/design/debugger.md`, "The MCP server", "Starting it"). On
//! Windows a running executable can be neither replaced nor removed, so a
//! server started from `target/debug/ns-next` would keep `cargo build`
//! from writing it; the copy lies where cargo never writes. The binary
//! knows nothing of the build; the repository's own concern is here. The
//! task builds nothing and starts nothing: it copies once, and a copy
//! already there stays, since a server may run from it -- refreshing it
//! is deleting `target/mcp/` while no server runs from it and running the
//! task again.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

/// The directory below the target directory that holds the copy.
const COPY_DIRECTORY: &str = "mcp";

/// The suffix of a copy being written, before it is renamed into place.
const PARTIAL_SUFFIX: &str = ".partial";

/// How long a partial copy stays before a task removes it: far longer
/// than a copy takes, so that no task removes one another task is still
/// writing, while one a task that died midway left goes at the next run.
const PARTIAL_STALE: Duration = Duration::from_secs(60);

/// The name of the executable, the build's and its copy's.
fn executable_name() -> String {
    format!("ns-next{}", std::env::consts::EXE_SUFFIX)
}

/// What the task found and did.
#[derive(Debug, PartialEq)]
enum Placed {
    /// The copy was written at this path.
    Copied(PathBuf),
    /// A copy was already at this path, and stays.
    AlreadyThere(PathBuf),
}

/// Copies the debug build to `target/mcp/` unless a copy is there, saying
/// which on the standard error.
pub(crate) fn place(root: &Path) -> ExitCode {
    match place_in(&target_directory(root)) {
        Ok(Placed::Copied(copy)) => {
            eprintln!(
                "cargo xtask mcp: copied the debug build to {}; (re)connect the MCP client \
                 (`/mcp`)",
                copy.display()
            );
            ExitCode::SUCCESS
        }
        Ok(Placed::AlreadyThere(copy)) => {
            eprintln!(
                "cargo xtask mcp: {} is already there and stays; to refresh it, delete {} \
                 while no server runs from it, run `cargo xtask mcp` again and reconnect the \
                 MCP client (`/mcp`)",
                copy.display(),
                copy.parent().unwrap_or(&copy).display()
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("cargo xtask mcp: {failure}");
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

/// Copies the debug build of the target directory `target` into its
/// copy's directory unless a copy is there; refuses when there is no
/// build, saying how to make one.
fn place_in(target: &Path) -> Result<Placed, String> {
    let build = target.join("debug").join(executable_name());
    if !build.is_file() {
        return Err(format!(
            "there is no build of ns-next at {}; build it with `cargo build -p ns-cli`,              then run `cargo xtask mcp` again",
            build.display()
        ));
    }
    place_copy(&build, &target.join(COPY_DIRECTORY))
}

/// Copies `build` into `directory` under the executable's name unless a
/// file is there already. The copy is written under a name of this
/// process's and renamed into place, so that a client never starts a half
/// written one.
fn place_copy(build: &Path, directory: &Path) -> Result<Placed, String> {
    let copy = directory.join(executable_name());
    if copy.exists() {
        return Ok(Placed::AlreadyThere(copy));
    }
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("creating {}: {error}", directory.display()))?;
    remove_stale_partials(directory);
    let partial = directory.join(format!(
        "{}.{}{PARTIAL_SUFFIX}",
        executable_name(),
        std::process::id()
    ));
    let written = std::fs::copy(build, &partial).and_then(|_| std::fs::rename(&partial, &copy));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "copying {} to {}: {error}",
            build.display(),
            copy.display()
        ));
    }
    Ok(Placed::Copied(copy))
}

/// Removes the partial copies in `directory` older than `PARTIAL_STALE`,
/// which a task that died between its copy and its rename left; a removal
/// that fails leaves the file.
fn remove_stale_partials(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let partial = entry
            .file_name()
            .to_string_lossy()
            .ends_with(PARTIAL_SUFFIX);
        let stale = std::fs::metadata(entry.path())
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > PARTIAL_STALE);
        if partial && stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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
        std::fs::read_dir(directory)
            .expect("read the directory")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect()
    }

    /// Without a copy, the build's bytes are copied under the
    /// executable's name, and nothing else is left in the directory.
    #[test]
    fn the_build_is_copied_when_no_copy_is_there() {
        let scratch = Scratch::new("copied");
        let build = scratch.0.join("build");
        std::fs::write(&build, "the build").expect("write the build");
        let directory = scratch.0.join("mcp");
        let placed = place_copy(&build, &directory).expect("a copy");
        let copy = directory.join(executable_name());
        assert_eq!(placed, Placed::Copied(copy.clone()));
        assert_eq!(
            std::fs::read_to_string(&copy).expect("read the copy"),
            "the build"
        );
        assert_eq!(names_in(&directory), [executable_name()]);
    }

    /// Without a build the task refuses, naming the command that makes
    /// one, and writes nothing.
    #[test]
    fn without_a_build_the_task_refuses() {
        let scratch = Scratch::new("refuses");
        let refusal = place_in(&scratch.0).expect_err("a refusal");
        assert!(refusal.contains("cargo build -p ns-cli"), "{refusal}");
        assert!(names_in(&scratch.0).is_empty(), "the task wrote something");
    }

    /// A partial copy a task that died midway left is removed; a fresh
    /// one, which another task may still be writing, stays.
    #[test]
    fn a_stale_partial_copy_is_removed_and_a_fresh_one_stays() {
        let scratch = Scratch::new("partials");
        let build = scratch.0.join("build");
        std::fs::write(&build, "the build").expect("write the build");
        let directory = scratch.0.join("mcp");
        std::fs::create_dir_all(&directory).expect("create the copy's directory");
        let stale = format!("{}.7{PARTIAL_SUFFIX}", executable_name());
        let fresh = format!("{}.8{PARTIAL_SUFFIX}", executable_name());
        std::fs::write(directory.join(&stale), "half").expect("write a stale partial copy");
        std::fs::write(directory.join(&fresh), "half").expect("write a fresh partial copy");
        let two_minutes_ago = std::time::SystemTime::now() - Duration::from_secs(120);
        std::fs::File::options()
            .write(true)
            .open(directory.join(&stale))
            .and_then(|file| file.set_modified(two_minutes_ago))
            .expect("age the stale partial copy");
        place_copy(&build, &directory).expect("a copy");
        let mut names = names_in(&directory);
        names.sort();
        let mut expected = vec![executable_name(), fresh];
        expected.sort();
        assert_eq!(names, expected);
    }

    /// A copy already there stays as it is, whatever the build holds: a
    /// server may run from it.
    #[test]
    fn a_copy_already_there_stays() {
        let scratch = Scratch::new("stays");
        let build = scratch.0.join("build");
        std::fs::write(&build, "the new build").expect("write the build");
        let directory = scratch.0.join("mcp");
        std::fs::create_dir_all(&directory).expect("create the copy's directory");
        let copy = directory.join(executable_name());
        std::fs::write(&copy, "an old build").expect("write an old copy");
        let placed = place_copy(&build, &directory).expect("an answer");
        assert_eq!(placed, Placed::AlreadyThere(copy.clone()));
        assert_eq!(
            std::fs::read_to_string(&copy).expect("read the copy"),
            "an old build"
        );
        assert_eq!(names_in(&directory), [executable_name()]);
    }
}
