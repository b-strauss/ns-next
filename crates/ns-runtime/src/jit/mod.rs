//! The baseline JIT: machine code for a body, written against the macro
//! assembler's idioms (`macro_assembler`) and entered and left through the
//! entry and exit stubs (`stubs`) (`docs/design/execution.md`, "Planned:
//! the baseline JIT").
//!
//! Only x64 has an encoder, so everything that emits or runs machine code
//! is compiled for x64 alone; elsewhere `available` answers `false` and
//! every body stays interpreted.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the JIT state and the compile trigger, which call these, are the next task's"
    )
)]

#[cfg(target_arch = "x86_64")]
pub(crate) mod macro_assembler;
#[cfg(target_arch = "x86_64")]
pub(crate) mod stubs;

/// Whether this build can compile bodies to machine code.
pub(crate) fn available() -> bool {
    cfg!(target_arch = "x86_64")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_jit_is_available_exactly_where_machine_code_is_emitted() {
        assert_eq!(super::available(), cfg!(target_arch = "x86_64"));
    }
}
