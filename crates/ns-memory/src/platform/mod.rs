//! The operating-system calls a reservation needs -- reserve, commit,
//! decommit, protect and release, plus the page-size and
//! reservation-granularity queries -- one implementation per platform behind
//! the same functions.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(unix)]
mod posix;
#[cfg(unix)]
pub(crate) use posix::*;
