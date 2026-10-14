//! The shipped core-library modules' compiled mixins, as bytes the binary
//! embeds (`docs/superpowers/specs/2026-10-14-precompiled-shipped-code-design.md`).
//! A codec of its own rather than the image's: the image decoder reads
//! untrusted input and refuses a mirror send (the VM mirror's lock (6):
//! `SendMirror` has no wire tag), while these bytes are the binary's own
//! and carry the kernel's mirror sends. The two share nothing.
//!
//! **The format.** `MAGIC`, `FORMAT_VERSION` (`u16`), the fingerprint
//! (`u64`), the module count (`u32`), then per module its name (`str`), its
//! section's byte length (`u32`) and the section, `Kernel` first and then
//! the `SHIPPED_MODULES` in order. A section is the symbol count (`u32`)
//! and each symbol (`str`), in order of first use, then the root mixin with
//! every nested one. A symbol in a mixin or a body is its `u32` index in its
//! section's table. Numbers are little-endian and fixed-width, a `str` its
//! `u32` byte length and its UTF-8, an `Option` a `0`/`1` byte and the
//! value. Its length lets a section be skipped and decoded on its own.
//!
//! **No verification.** Bytes that do not decode are a defect of the
//! binary, as a shipped module that does not parse is, and panic; the
//! decoder has no error type.

use std::rc::Rc;

use crate::compiler::Lowering;
use crate::interpreter::core_library::SHIPPED_MODULES;
use crate::interpreter::Interpreter;
use crate::mixin::Mixin;

mod bytes;
// Read by the tests now, and by the boot from the next step on.
#[cfg_attr(not(test), expect(dead_code))]
mod decode;
mod encode;

pub(crate) use encode::encode;

use bytes::ByteReader;

pub(crate) const MAGIC: [u8; 4] = *b"NSSC";
pub(crate) const FORMAT_VERSION: u16 = 1;

// `SHIPPED_CODE_FINGERPRINT` is the fingerprint of the sources and the
// compiler the artifact was generated from, which the boot compares against
// the binary's own; `CORE_LIBRARY_FINGERPRINT` covers the core library alone.
include!(concat!(env!("OUT_DIR"), "/fingerprints.rs"));

/// The artifact `cargo xtask shipped-code` wrote, embedded when the crate
/// was built; empty when the tree had none.
// The boot reads it in a later task.
#[expect(dead_code)]
pub(crate) fn embedded() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/shipped-code.bin"))
}

/// Where a boot takes the shipped modules' compiled mixins from: the
/// artifact the binary embeds, or their source, compiled and checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShippedCodePath {
    // The boot chooses it from the next step of the precompiled shipped code on.
    #[expect(dead_code)]
    Embedded,
    Source,
}

/// The shipped modules' names, in the artifact's order: `Kernel`, then
/// every `SHIPPED_MODULES` name.
pub(crate) fn module_names() -> impl Iterator<Item = &'static str> {
    std::iter::once("Kernel").chain(SHIPPED_MODULES.iter().map(|shipped| shipped.name))
}

/// The artifact: every shipped module compiled through the source path,
/// checks included, with the optimized lowering, then encoded. For
/// `cargo xtask shipped-code`.
#[doc(hidden)]
pub fn generate() -> Vec<u8> {
    let mut interpreter = Interpreter::bootstrap_on(Lowering::Optimized, ShippedCodePath::Source);
    encode(&interpreter.shipped_root_mixins(), SHIPPED_CODE_FINGERPRINT)
}

/// The root mixin of the module `name` in the artifact `bytes`.
// Read by the tests now, and by the boot from the next step on.
#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn decode_module(bytes: &[u8], name: &str) -> Rc<Mixin> {
    let mut reader = ByteReader::new(bytes);
    assert!(
        reader.raw(MAGIC.len()) == MAGIC,
        "internal: the embedded shipped code has no magic"
    );
    let version = reader.u16();
    assert!(
        version == FORMAT_VERSION,
        "internal: the embedded shipped code is format {version}, not {FORMAT_VERSION}"
    );
    // The fingerprint, which the boot compares before it decodes a module.
    reader.u64();
    for _ in 0..reader.length() {
        let module = reader.str();
        let length = reader.length();
        let section = reader.raw(length);
        if module == name {
            return decode::decode_section(section);
        }
    }
    panic!("internal: the embedded shipped code holds no module {name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Instruction;

    fn decoded_modules(bytes: &[u8]) -> Vec<(&'static str, Rc<Mixin>)> {
        module_names()
            .map(|name| (name, decode_module(bytes, name)))
            .collect()
    }

    #[test]
    fn the_two_fingerprints_differ() {
        // The code's covers the Rust sources too; equal, it would cover the
        // core library alone and a compiler change would leave a stale
        // artifact in use.
        assert_ne!(SHIPPED_CODE_FINGERPRINT, CORE_LIBRARY_FINGERPRINT);
    }

    #[test]
    fn generated_shipped_code_round_trips() {
        let generated = generate();
        let decoded = decoded_modules(&generated);
        assert_eq!(encode(&decoded, SHIPPED_CODE_FINGERPRINT), generated);
    }

    #[test]
    fn decoded_shipped_code_keeps_the_mirror_sends_and_fast_paths() {
        let generated = generate();
        let kernel = decode_module(&generated, "Kernel");
        let object = kernel
            .nested
            .iter()
            .find(|nested| nested.name.as_str() == "Object")
            .expect("Kernel.ns declares Object");
        let mut mirror_sends = 0;
        let mut fast_paths = 0;
        for method in object.compiled_methods() {
            fast_paths += usize::from(method.fast_path.is_some());
            mirror_sends += method
                .code
                .instructions()
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::SendMirror { .. }))
                .count();
        }
        assert!(mirror_sends > 0 && fast_paths > 0);
        assert!(kernel.shipped && kernel.declaration.is_none());
    }

    #[test]
    fn generated_shipped_code_is_reproducible() {
        assert_eq!(generate(), generate());
    }
}
