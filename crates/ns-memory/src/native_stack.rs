//! The running thread's native stack: how much of it is left below the
//! caller. Compiled frames keep their return addresses on the native stack
//! of the thread that runs the interpreter, so an entry into machine code
//! checks there is room for the deepest recursion the depth limit allows
//! before it enters (docs/design/execution.md, "The native stack carries
//! the return addresses").

use std::cell::OnceCell;

use crate::platform;

thread_local! {
    /// The running thread's lowest usable stack address, asked of the
    /// system once per thread: it does not change while the thread lives,
    /// and on Linux the main thread's answer reads `/proc/self/maps`.
    static LOWEST_USABLE_ADDRESS: OnceCell<Option<usize>> = const { OnceCell::new() };
}

/// The bytes of the running thread's native stack between the caller's
/// frame and the stack's lowest usable address, or `None` where the
/// platform does not say -- which the JIT answers by running interpreted.
/// Windows reports its guard pages and the guarantee for the overflow
/// handler, which are left out; a system whose report includes a guard
/// page (glibc before 2.27) answers that much more, which a caller's
/// margin covers. A Linux main thread under a large or unlimited stack
/// limit reports a stack reaching down to the next mapping, the kernel's
/// guard gap included, so it answers up to that gap (1 MiB by default)
/// more -- of a stack far larger than any need the JIT has. The stack
/// grows down on every platform the code zone supports.
pub fn remaining_bytes() -> Option<usize> {
    let lowest = LOWEST_USABLE_ADDRESS
        .with(|lowest| *lowest.get_or_init(platform::thread_stack_lowest_usable_address))?;
    current_position().checked_sub(lowest)
}

/// An address within the caller's frame: a local of a frame of its own,
/// never inlined, so that it lies below whatever the caller holds.
#[inline(never)]
fn current_position() -> usize {
    let marker = 0u8;
    std::ptr::from_ref(std::hint::black_box(&marker)).addr()
}

#[cfg(test)]
mod tests {
    use super::remaining_bytes;

    /// The bytes `remaining_bytes` answers `depth` frames of at least 256
    /// bytes each below the caller.
    #[inline(never)]
    fn remaining_at_depth(depth: u32) -> usize {
        let padding = std::hint::black_box([0u8; 256]);
        let remaining = if depth == 0 {
            remaining_bytes().expect("known on every platform the suite runs on")
        } else {
            remaining_at_depth(depth - 1)
        };
        std::hint::black_box(&padding);
        remaining
    }

    #[test]
    fn the_remaining_native_stack_is_known_and_shrinks_with_depth() {
        let here = remaining_bytes().expect("known on every platform the suite runs on");
        let deeper = remaining_at_depth(16);
        assert!(deeper < here, "{deeper} below {here}");
        assert!(here - deeper >= 16 * 256, "each frame holds its padding");
    }

    #[test]
    fn a_thread_knows_the_stack_it_was_given() {
        let bytes = 1 << 20;
        let remaining = std::thread::Builder::new()
            .stack_size(bytes)
            .spawn(remaining_bytes)
            .expect("a thread")
            .join()
            .expect("joined")
            .expect("known on every platform the suite runs on");
        // The system rounds a thread's stack up and keeps a guard; the
        // answer is that stack's, not the spawning thread's.
        assert!(remaining > bytes / 2, "{remaining} of {bytes}");
        assert!(remaining <= 2 * bytes, "{remaining} of {bytes}");
    }
}
