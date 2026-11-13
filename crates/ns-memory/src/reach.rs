//! The record of what a test's process reaches, which decides the tests
//! the stress and Docker gates leave out (`docs/design/README.md`,
//! "Gates"; `cargo xtask unreached-tests`).
//!
//! A stress switch changes only what the runtime does once it runs, the
//! wide encoding only what the compiler builds, and a platform only the
//! code that asks it something; a test whose process reaches none of these
//! answers the same under every switch and on every platform, so running
//! it again there checks nothing. Whether it reaches them is measured, not
//! judged by reading the test: in a debug build, with `NS_NEXT_RECORD_REACH`
//! naming a directory, the first `record` of each kind in a process writes
//! one marker there, named after the test nextest says is running
//! (`NEXTEST_BINARY_ID`, `NEXTEST_TEST_NAME`), which a child process the
//! test starts -- an `ns-next`, say -- inherits. It lives here, the lowest
//! crate, so that the memory layer records its own calls into the system
//! and every crate above reaches it. A release build records nothing, and
//! `record` compiles to nothing there.

/// The variable naming the directory the markers are written into.
pub const VARIABLE: &str = "NS_NEXT_RECORD_REACH";

/// What a process can reach that a gate's choice of tests depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reach {
    /// The runtime: an interpreter made, or a variable read that changes
    /// how the runtime runs -- a stress switch other than the wide
    /// encoding's, or `NS_NEXT_NO_JIT`.
    Runtime,
    /// The compiler's encoding: `NS_NEXT_STRESS_WIDE` read, which every
    /// body the compiler builds asks.
    Compiler,
    /// Code that answers differently by platform: the memory layer's calls
    /// into the system, the JIT's host convention, and the host's paths
    /// and processor.
    Platform,
}

impl Reach {
    /// Every kind, in the order a list names them.
    pub const ALL: [Reach; 3] = [Reach::Runtime, Reach::Compiler, Reach::Platform];

    /// The kind's name in a marker and in the list of unreached tests.
    pub fn name(self) -> &'static str {
        match self {
            Reach::Runtime => "runtime",
            Reach::Compiler => "compiler",
            Reach::Platform => "platform",
        }
    }

    /// The kind `name` names, the inverse of `name`.
    pub fn named(name: &str) -> Option<Reach> {
        Reach::ALL.into_iter().find(|reach| reach.name() == name)
    }
}

/// Records that the running process reached `reach`: a marker written the
/// first time a process records each kind, in a debug build run with
/// `NS_NEXT_RECORD_REACH` set; nothing otherwise. After the first, a call
/// is one relaxed load, so it may stand where a body is compiled or memory
/// committed, though not on the run loop's path.
#[inline]
pub fn record(reach: Reach) {
    #[cfg(debug_assertions)]
    markers::record(reach);
    #[cfg(not(debug_assertions))]
    let _ = reach;
}

#[cfg(debug_assertions)]
mod markers {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{Reach, VARIABLE};

    /// Whether this process has recorded each kind, by `Reach as usize`.
    static RECORDED: [AtomicBool; 3] = [const { AtomicBool::new(false) }; 3];

    pub(super) fn record(reach: Reach) {
        let recorded = &RECORDED[reach as usize];
        if recorded.load(Ordering::Relaxed) || recorded.swap(true, Ordering::Relaxed) {
            return;
        }
        write_marker(reach);
    }

    /// The marker of `reach` for the running test, unless the record is
    /// off or no test runs. Its name is a hash of the test's binary and
    /// name, so that the test and the processes it starts write the same
    /// file, which only the first creates; its one line names all three,
    /// which the hash cannot be read back into. A marker that cannot be
    /// written fails the test, since a record missing one would let a gate
    /// leave out a test it must run.
    #[cold]
    fn write_marker(reach: Reach) {
        let Some(directory) = std::env::var_os(VARIABLE) else {
            return;
        };
        let (Ok(binary), Ok(test)) = (
            std::env::var("NEXTEST_BINARY_ID"),
            std::env::var("NEXTEST_TEST_NAME"),
        ) else {
            return;
        };
        let path = std::path::Path::new(&directory).join(format!(
            "{:016x}-{}",
            fnv1a(&format!("{binary}\t{test}")),
            reach.name()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => file
                .write_all(format!("{binary}\t{test}\t{}\n", reach.name()).as_bytes())
                .unwrap_or_else(|error| panic!("{VARIABLE}: write {}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("{VARIABLE}: create {}: {error}", path.display()),
        }
    }

    /// FNV-1a over `text`, written out: no hasher of the standard library
    /// promises the same answer in every process.
    fn fnv1a(text: &str) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        text.bytes().fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Reach;

    #[test]
    fn a_kind_is_named_by_its_name() {
        for reach in Reach::ALL {
            assert_eq!(Reach::named(reach.name()), Some(reach));
        }
        assert_eq!(Reach::named("everything"), None);
    }
}
