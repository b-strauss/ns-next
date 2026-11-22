//! The directory in which the server keeps its files -- the shipped
//! sources its sessions name (`shipped_sources.rs`) and their trace files
//! (`trace_files.rs`) -- which is the user's own. Nothing is kept in the
//! shared temporary directory, where another user could put a file of
//! their own at the path first: sources the server would show, or a trace
//! it would read.

use std::path::PathBuf;

/// The directory in the user's own local data directory that holds what
/// the server keeps.
const OWN_DIRECTORY: &str = "ns-next";

/// The directory the server keeps its files in: on Windows `ns-next` in
/// the local application data directory (`%LOCALAPPDATA%`), elsewhere in
/// the user's cache directory (`$XDG_CACHE_HOME`, or `~/.cache`). A
/// variable naming a relative path names no directory, since it would
/// depend on where the server was started.
pub(crate) fn own_directory() -> Result<PathBuf, String> {
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
