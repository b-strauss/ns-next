//! The server's trace files: a traced session's child writes its
//! `--trace-file` into a file the server created in a directory it
//! claimed, `traces/<server>/session-<n>.trace` in the user's own
//! directory (`own_directory.rs`), never the shared temporary directory,
//! where another user could put a file at the path first.
//!
//! A server claims its directory with a lock it holds for its whole life
//! on a file of its own beside it, `traces/<server>.lock`: the operating
//! system releases an exclusive lock when its process ends, however it
//! ends, on Windows (`LockFileEx`) and POSIX (`flock`) alike, so a server
//! that starts knows another is gone when it can take that lock itself,
//! with no age guessing at whether a server that ran for days still runs.
//! The lock is not on a trace file, since a Windows byte-range lock there
//! would refuse the child's writes.
//!
//! The rule rests on every server that shares the directory seeing the
//! others' locks. A home on a network filesystem whose locks are local to
//! each client -- Linux NFS mounted with `local_lock=flock`, some CIFS
//! setups -- breaks it: a server on one machine can take the lock of a
//! live server on another and remove its files. Nothing in `std` tells
//! such a mount apart, so the own directory is to lie on a filesystem
//! whose locks every client sees.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::own_directory::own_directory;
use crate::debug::Token;

/// The directory below the server's own that holds every server's trace
/// directory and lock file.
const TRACES_DIRECTORY: &str = "traces";

/// The extension of a server's lock file.
const LOCK_EXTENSION: &str = "lock";

/// How many names a claim tries. A name is given up only when another
/// server's cleanup raced its claim, which resolves at once on a
/// filesystem whose locks work; one where every attempt fails must not
/// hold the server's start forever, and the server then runs without
/// trace files.
const CLAIM_ATTEMPTS: usize = 16;

/// The mode `traces/` is created with on POSIX: the user's alone, so that
/// no other user reads the traces below it -- bodies' names, bytecode,
/// machine code -- in a cache directory others may read.
#[cfg(unix)]
const TRACES_MODE: u32 = 0o700;

/// `CreateFileW`'s `dwShareMode` flag that lets other handles read the
/// file: the child's, and a reader's.
/// <https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew>
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x1;

/// `CreateFileW`'s `dwShareMode` flag that lets other handles write the
/// file -- the child's `File::create`, which truncates and writes.
/// <https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew>
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x2;

/// The directory a server claimed for its trace files, its lock held
/// while it lives. Dropped -- once the server and every file in it are
/// done -- it removes the directory and then its lock file, which
/// outlives it, so that no directory is ever found without its lock
/// while its server lives.
pub(crate) struct TraceDirectory {
    directory: PathBuf,
    lock_path: PathBuf,
    /// Held for the lock, which ends with the handle.
    _lock: File,
}

impl TraceDirectory {
    /// Removes the trace directories of servers that are gone, then
    /// claims a directory of this server's under a name no other server
    /// has: `<process id>-<32 hex digits from the operating system's
    /// random source>`, so that servers of several clients, of several
    /// users' containers sharing a home, or a process id reused never
    /// collide.
    pub(crate) fn claim() -> Result<TraceDirectory, String> {
        let own = own_directory()?;
        std::fs::create_dir_all(&own)
            .map_err(|error| format!("creating {}: {error}", own.display()))?;
        let traces = own.join(TRACES_DIRECTORY);
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(TRACES_MODE);
        }
        builder
            .create(&traces)
            .map_err(|error| format!("creating {}: {error}", traces.display()))?;
        remove_dead(&traces);
        let mut last_reason = String::new();
        for _ in 0..CLAIM_ATTEMPTS {
            let token = Token::generate()
                .map_err(|error| format!("naming the trace directory: {error}"))?;
            let name = format!("{}-{}", std::process::id(), token.hex());
            let lock_path = traces.join(format!("{name}.{LOCK_EXTENSION}"));
            let lock = match File::options()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(lock) => lock,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_reason = format!("{} exists already", lock_path.display());
                    continue;
                }
                Err(error) => return Err(format!("creating {}: {error}", lock_path.display())),
            };
            match lock.try_lock() {
                Ok(()) => {}
                // Another server's cleanup holds it, between this one's
                // creation and its lock, and removes it.
                Err(TryLockError::WouldBlock) => {
                    last_reason = format!("{} is locked by another", lock_path.display());
                    continue;
                }
                Err(TryLockError::Error(error)) => {
                    drop(lock);
                    let _ = std::fs::remove_file(&lock_path);
                    return Err(format!("locking {}: {error}", lock_path.display()));
                }
            }
            // Another server's cleanup locked and removed it in between:
            // the lock this one holds guards no name.
            if !lock_path.is_file() {
                last_reason = format!("{} was removed once locked", lock_path.display());
                continue;
            }
            let claimed = TraceDirectory {
                directory: traces.join(&name),
                lock_path,
                _lock: lock,
            };
            std::fs::create_dir(&claimed.directory)
                .map_err(|error| format!("creating {}: {error}", claimed.directory.display()))?;
            return Ok(claimed);
        }
        Err(format!(
            "no name claimed in {CLAIM_ATTEMPTS} attempts: {last_reason}"
        ))
    }

    /// Creates the trace file of the session `session_number`, which the
    /// child is to write, and holds it open. On Windows the handle shares
    /// reading and writing but not deletion, so that the child's
    /// `File::create` truncates and writes it while no other process can
    /// delete or rename it; on POSIX the handle reads on whatever is done
    /// to the path.
    pub(crate) fn create(
        self: &Arc<TraceDirectory>,
        session_number: u32,
    ) -> Result<TraceFile, String> {
        let path = self
            .directory
            .join(format!("session-{session_number}.trace"));
        let mut options = File::options();
        // A session's number is never given twice, so a file at the path
        // is no file of this server's.
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        let file = options
            .open(&path)
            .map_err(|error| format!("creating the trace file {}: {error}", path.display()))?;
        Ok(TraceFile {
            path,
            file: Some(file),
            _directory: Arc::clone(self),
        })
    }
}

impl Drop for TraceDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// A session's trace file, held open; dropped, it is closed and then
/// deleted. It keeps its directory, which is removed only once every file
/// in it is gone.
pub(crate) struct TraceFile {
    path: PathBuf,
    /// `None` only in `drop`, which closes it before the deletion: on
    /// Windows this very handle refuses the deletion while it is open.
    file: Option<File>,
    _directory: Arc<TraceDirectory>,
}

impl TraceFile {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TraceFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Removes, in `traces`, what servers that are gone left: for each lock
/// file this server can lock itself, its directory and then the lock file,
/// and every directory without a lock file beside it, which a live
/// server's directory always has, its lock created first and removed
/// last. Any name is accepted, whatever its form; what cannot be removed
/// is left.
fn remove_dead(traces: &Path) {
    let Ok(entries) = std::fs::read_dir(traces) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|extension| extension != LOCK_EXTENSION)
            || !path.is_file()
        {
            continue;
        }
        let Ok(lock) = File::options().read(true).write(true).open(&path) else {
            continue;
        };
        if lock.try_lock().is_ok() {
            // Removed while the lock is held, so that a server claiming
            // the name meanwhile finds it taken or gone.
            let _ = std::fs::remove_dir_all(path.with_extension(""));
            let _ = std::fs::remove_file(&path);
        }
    }
    let Ok(entries) = std::fs::read_dir(traces) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let mut lock_name = entry.file_name();
        lock_name.push(format!(".{LOCK_EXTENSION}"));
        if !traces.join(lock_name).exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}
