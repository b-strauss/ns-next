//! The five operating-system calls a reservation needs, one implementation
//! per platform behind the same functions.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
#[allow(unused_imports)] // nothing calls the layer until `Reservation`
pub(crate) use windows::*;

#[cfg(unix)]
mod posix;
#[cfg(unix)]
#[allow(unused_imports)] // nothing calls the layer until `Reservation`
pub(crate) use posix::*;
