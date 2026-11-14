//! The operating system's cryptographic random source, for the debugger's
//! token and nothing else: the runtime's own generators are not secrets and
//! never come through here.

use crate::reach;

/// The operating system refused to fill a buffer; `code` is its error number
/// (`NTSTATUS` on Windows, `errno` elsewhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomError {
    code: i32,
}

impl std::fmt::Display for RandomError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the operating system's random source failed (error {})",
            self.code
        )
    }
}

impl std::error::Error for RandomError {}

/// Fills `bytes` from the operating system's cryptographic random source.
/// Meant for the debugger's token only; it either fills the whole buffer or
/// answers the system's error, never a partly filled buffer as success.
pub fn fill_random(bytes: &mut [u8]) -> Result<(), RandomError> {
    reach::record(reach::Reach::Platform);
    fill(bytes)
}

#[cfg(windows)]
fn fill(bytes: &mut [u8]) -> Result<(), RandomError> {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    // The call takes a 32-bit length, so a larger buffer goes in pieces.
    for chunk in bytes.chunks_mut(u32::MAX as usize) {
        // SAFETY: `chunk` is a live, writable range of exactly the length
        // passed; a null algorithm handle with the system-preferred flag is
        // the documented form, which opens no provider.
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                chunk.as_mut_ptr(),
                u32::try_from(chunk.len()).expect("a chunk fits the call's length"),
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        // NTSTATUS success is zero; every failure has the sign bit set.
        if status != 0 {
            return Err(RandomError { code: status });
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn fill(bytes: &mut [u8]) -> Result<(), RandomError> {
    let mut filled = 0;
    while filled < bytes.len() {
        let rest = &mut bytes[filled..];
        // SAFETY: `rest` is a live, writable range of exactly the length
        // passed, and flags 0 asks for the blocking urandom source.
        let count = unsafe { libc::getrandom(rest.as_mut_ptr().cast(), rest.len(), 0) };
        if count < 0 {
            let code = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default();
            // A signal interrupted the call before it read anything.
            if code == libc::EINTR {
                continue;
            }
            return Err(RandomError { code });
        }
        // A large request may be answered short; the loop asks for the rest.
        filled += count.cast_unsigned();
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn fill(bytes: &mut [u8]) -> Result<(), RandomError> {
    /// `getentropy` refuses a request over 256 bytes.
    const LARGEST_REQUEST: usize = 256;
    for chunk in bytes.chunks_mut(LARGEST_REQUEST) {
        // SAFETY: `chunk` is a live, writable range of exactly the length
        // passed, which is within the call's 256-byte limit.
        let status = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) };
        if status != 0 {
            let code = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default();
            return Err(RandomError { code });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fill_random;

    #[test]
    fn fill_random_fills_a_large_buffer() {
        let mut bytes = [0u8; 1000];
        fill_random(&mut bytes).expect("the operating system answers");
        assert!(bytes.iter().any(|byte| *byte != 0));
    }

    #[test]
    fn two_fills_differ() {
        let mut first = [0u8; 16];
        let mut second = [0u8; 16];
        fill_random(&mut first).expect("the operating system answers");
        fill_random(&mut second).expect("the operating system answers");
        assert_ne!(first, second);
    }
}
